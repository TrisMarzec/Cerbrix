//! The problems list and the acknowledging and closing of problems.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use cerbrix_common::config::Severity;
use cerbrix_common::validate_host_name;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{ApiError, ApiJson, Entry, Page, invalid, query_text};
use crate::api::AppState;
use crate::auth::{Operator, Viewer};
use crate::gitops::replace_control_by_space;
use crate::state::{Problem, ProblemQuery, ProblemStateFilter, StateError};
use crate::view::dashboard::{DashboardRef, dashboards_of_hosts};

pub(super) fn entries() -> Vec<Entry> {
    vec![
        Entry::new("GET", "/api/v1/problems", list),
        Entry::new("GET", "/api/v1/problems/{id}", show),
        Entry::new("POST", "/api/v1/problems/{id}/ack", acknowledge),
        Entry::new("POST", "/api/v1/problems/{id}/close", close),
    ]
}

/// The longest search text taken.
const MAX_SEARCH_CHARS: usize = 200;

/// The longest group name (the length of a valid one).
const MAX_GROUP_CHARS: usize = 63;

/// The longest dashboard id taken.
const MAX_DASHBOARD_CHARS: usize = 200;

fn store_failed(what: &str, e: &StateError) -> ApiError {
    tracing::error!(error = %e, "{what}");
    ApiError::internal()
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such problem")
}

fn conflict() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "conflict",
        "the problem is already resolved",
    )
}

/// A malformed id cannot name a problem.
fn parse_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| not_found())
}

#[derive(Deserialize)]
struct ProblemParams {
    state: Option<String>,
    severity: Option<String>,
    host: Option<String>,
    group: Option<String>,
    dashboard: Option<String>,
    search: Option<String>,
}

fn parse_severity(raw: &str) -> Result<Severity, ApiError> {
    match raw {
        "info" => Ok(Severity::Info),
        "warning" => Ok(Severity::Warning),
        "high" => Ok(Severity::High),
        "critical" => Ok(Severity::Critical),
        _ => Err(invalid("severity must be info, warning, high or critical")),
    }
}

/// One page of problems and the dashboards of the hosts on it.
#[derive(Debug, Serialize)]
struct ProblemsListed {
    items: Vec<Problem>,
    total: i64,
    /// For every host name in `items`, the dashboards that select it (sorted by name; empty
    /// for a host in none, an inactive host included).
    dashboards: BTreeMap<String, Vec<DashboardRef>>,
}

async fn list(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    page: Page,
    params: Result<Query<ProblemParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ProblemsListed>, ApiError> {
    let Query(params) = params.map_err(|_| invalid("the query parameters are malformed"))?;
    for (value, max, message) in [
        (
            &params.search,
            MAX_SEARCH_CHARS,
            "search must have at most 200 characters and no control character",
        ),
        (
            &params.group,
            MAX_GROUP_CHARS,
            "group must have at most 63 characters and no control character",
        ),
        (
            &params.dashboard,
            MAX_DASHBOARD_CHARS,
            "dashboard must have at most 200 characters and no control character",
        ),
    ] {
        if let Some(text) = value {
            query_text(text, max, message)?;
        }
    }
    let state = params
        .state
        .as_deref()
        .map(|s| {
            s.parse::<ProblemStateFilter>()
                .map_err(|()| invalid("state must be open, resolved or all"))
        })
        .transpose()?
        .unwrap_or(ProblemStateFilter::Open);
    let min_severity = params.severity.as_deref().map(parse_severity).transpose()?;
    // The hosts the dashboard selects; an unknown dashboard is a mistake of the caller.
    let dashboard_hosts = match &params.dashboard {
        Some(name) => Some(
            app.store
                .dashboard_host_ids(name)
                .await
                .map_err(|e| store_failed("looking up a dashboard failed", &e))?
                .ok_or_else(|| invalid("no dashboard with this id"))?,
        ),
        None => None,
    };
    // A name that is not a valid host name matches no host, and nothing is asked of the store.
    if params
        .host
        .as_deref()
        .is_some_and(|h| validate_host_name(h).is_err())
    {
        return Ok(Json(ProblemsListed {
            items: Vec::new(),
            total: 0,
            dashboards: BTreeMap::new(),
        }));
    }
    let query = ProblemQuery {
        state,
        min_severity,
        host: params.host,
        group: params.group,
        dashboard: params.dashboard,
        search: params.search,
        limit: page.limit,
        offset: page.offset,
    };
    let (items, total) = app
        .store
        .problems_page(&query, dashboard_hosts.as_deref())
        .await
        .map_err(|e| store_failed("listing problems failed", &e))?;
    // Two more store calls whatever the page holds: every dashboard and the active hosts;
    // membership is `selected_hosts`, applied to the hosts on this page.
    let mut dashboards: BTreeMap<String, Vec<DashboardRef>> =
        items.iter().map(|p| (p.host.clone(), Vec::new())).collect();
    if !items.is_empty() {
        let all_dashboards = app
            .store
            .dashboards()
            .await
            .map_err(|e| store_failed("reading dashboards failed", &e))?;
        let active = app
            .store
            .active_hosts()
            .await
            .map_err(|e| store_failed("reading hosts failed", &e))?;
        let on_page: Vec<_> = active
            .into_iter()
            .filter(|h| dashboards.contains_key(&h.name))
            .collect();
        dashboards.extend(dashboards_of_hosts(&all_dashboards, &on_page));
    }
    Ok(Json(ProblemsListed {
        items,
        total,
        dashboards,
    }))
}

async fn show(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    Path(id): Path<String>,
) -> Result<Json<Problem>, ApiError> {
    let id = parse_id(&id)?;
    app.store
        .problem(id)
        .await
        .map_err(|e| store_failed("reading a problem failed", &e))?
        .map(Json)
        .ok_or_else(not_found)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommentBody {
    #[serde(default)]
    comment: String,
}

/// A comment as it is stored: control characters replaced by spaces, trimmed. The store
/// cuts it to 1,000 characters.
fn clean_comment(raw: &str) -> String {
    replace_control_by_space(raw).trim().to_owned()
}

async fn acknowledge(
    State(app): State<AppState>,
    Operator(user): Operator,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<CommentBody>,
) -> Result<Json<Problem>, ApiError> {
    let id = parse_id(&id)?;
    let problem = app
        .store
        .acknowledge_open_problem(
            id,
            &user.name,
            &clean_comment(&body.comment),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(|e| match e {
            StateError::NotFound => not_found(),
            StateError::Conflict(_) => conflict(),
            e => store_failed("acknowledging a problem failed", &e),
        })?;
    tracing::info!(user = %user.name, problem = %id, "problem acknowledged");
    Ok(Json(problem))
}

async fn close(
    State(app): State<AppState>,
    Operator(user): Operator,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<CommentBody>,
) -> Result<Json<Problem>, ApiError> {
    let id = parse_id(&id)?;
    let comment = clean_comment(&body.comment);
    if comment.is_empty() {
        return Err(invalid("a comment is required to close a problem"));
    }
    let problem = app
        .store
        .close_problem(
            id,
            &user.name,
            &comment,
            OffsetDateTime::now_utc(),
            app.queue_mail,
        )
        .await
        .map_err(|e| match e {
            StateError::NotFound => not_found(),
            StateError::Conflict(_) => conflict(),
            e => store_failed("closing a problem failed", &e),
        })?;
    tracing::info!(user = %user.name, problem = %id, "problem closed");
    Ok(Json(problem))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use cerbrix_common::config::Severity;
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::super::testing::*;
    use crate::alerting::{Evaluation, OpenProblem, ProblemKind};
    use crate::auth::Role;
    use crate::gitops::{compile, read_tree};

    const T0_MS: i64 = 1_800_000_000_000;

    /// Opens a problem on the host `name` (created when missing) and returns its id.
    async fn open(
        fx: &Fixture,
        name: &str,
        trigger: &str,
        severity: Severity,
        at_ms: i64,
        queue_mail: bool,
    ) -> Uuid {
        let host = match fx.store.host_by_name(name).await.unwrap() {
            Some(h) => h.id,
            None => fx.store.create_host(name).await.unwrap().id,
        };
        let o = OpenProblem {
            trigger: trigger.to_owned(),
            instance: String::new(),
            severity,
            at_ms,
            summary: format!("{trigger} is bad"),
            kind: ProblemKind::Compare,
        };
        fx.store
            .evaluate_host(host, queue_mail, |_| Evaluation {
                open: vec![o],
                ..Evaluation::default()
            })
            .await
            .unwrap();
        sqlx::query_scalar(
            "SELECT id FROM problems WHERE host_id = $1 AND trigger_name = $2 \
             AND resolved_at IS NULL",
        )
        .bind(host)
        .bind(trigger)
        .fetch_one(&fx.pool)
        .await
        .unwrap()
    }

    async fn get(fx: &Fixture, role: Role, uri: &str) -> Reply {
        let token = fx.session(role).await;
        fx.call("GET", uri, Some(&token), &[], None).await
    }

    async fn post(fx: &Fixture, role: Role, uri: &str, body: &str) -> Reply {
        let token = fx.session(role).await;
        fx.call("POST", uri, Some(&token), &[], Some(body)).await
    }

    fn triggers(reply: &Reply) -> Vec<String> {
        reply.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["trigger"].as_str().unwrap().to_owned())
            .collect()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_defaults_to_open(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let old = open(&fx, "web", "Old", Severity::High, T0_MS, false).await;
        open(&fx, "web", "Crit", Severity::Critical, T0_MS + 1, false).await;
        let reply = post(
            &fx,
            Role::Operator,
            &format!("/api/v1/problems/{old}/close"),
            r#"{"comment": "gone"}"#,
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);

        let reply = get(&fx, Role::Viewer, "/api/v1/problems").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(triggers(&reply), ["Crit"]);
        assert_eq!(reply.json()["total"], 1);
        let first = &reply.json()["items"][0];
        assert_eq!(first["host"], "web");
        assert_eq!(first["severity"], "critical");
        assert_eq!(first["kind"], "compare");
        assert!(first["resolved_at"].is_null());

        let reply = get(&fx, Role::Viewer, "/api/v1/problems?state=resolved").await;
        assert_eq!(triggers(&reply), ["Old"]);
        assert_eq!(
            reply.json()["items"][0]["resolve_reason"],
            "closed_by_operator"
        );
        let reply = get(&fx, Role::Viewer, "/api/v1/problems?state=all&limit=1").await;
        assert_eq!(
            (triggers(&reply), reply.json()["total"].as_i64()),
            (vec!["Crit".to_owned()], Some(2))
        );
    }

    async fn add_dashboard(fx: &Fixture, id: &str, body: &str) {
        sqlx::query(
            "INSERT INTO config_documents (kind, id, source_file, body) VALUES ('dashboard', $1, $2, $3::jsonb)",
        )
        .bind(id)
        .bind(format!("dashboards/{id}.json"))
        .bind(body)
        .execute(&fx.pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn open_problems_are_newest_first_within_a_severity(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        open(&fx, "web", "Old high", Severity::High, T0_MS, false).await;
        open(&fx, "web", "New high", Severity::High, T0_MS + 5000, false).await;
        open(&fx, "web", "Mid high", Severity::High, T0_MS + 2000, false).await;
        open(
            &fx,
            "web",
            "Old crit",
            Severity::Critical,
            T0_MS - 9000,
            false,
        )
        .await;
        let acked = open(
            &fx,
            "web",
            "Newest acked",
            Severity::Critical,
            T0_MS + 9000,
            false,
        )
        .await;
        let reply = post(
            &fx,
            Role::Operator,
            &format!("/api/v1/problems/{acked}/ack"),
            "{}",
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        let reply = get(&fx, Role::Viewer, "/api/v1/problems").await;
        assert_eq!(
            triggers(&reply),
            [
                "Old crit",
                "New high",
                "Mid high",
                "Old high",
                "Newest acked"
            ]
        );
        // Resolved problems and all: newest first, as before.
        let reply = get(&fx, Role::Viewer, "/api/v1/problems?state=all").await;
        assert_eq!(triggers(&reply)[0], "Newest acked");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn problems_list_names_dashboards_of_its_hosts(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        open(&fx, "a-host", "Cpu", Severity::Critical, T0_MS + 3, false).await;
        open(&fx, "b-host", "Cpu", Severity::Critical, T0_MS + 2, false).await;
        // On another page: the newest two are the page.
        open(&fx, "c-host", "Cpu", Severity::Critical, T0_MS + 1, false).await;
        sqlx::query("UPDATE hosts SET groups = ARRAY['prod'] WHERE name = 'a-host'")
            .execute(&pool)
            .await
            .unwrap();
        add_dashboard(&fx, "z", r#"{"name":"Zulu","hosts":{"groups":["prod"]}}"#).await;
        add_dashboard(
            &fx,
            "y",
            r#"{"name":"Alpha","hosts":{"names":["a-host","c-host"]}}"#,
        )
        .await;
        let body = get(&fx, Role::Viewer, "/api/v1/problems?limit=2")
            .await
            .json();
        assert_eq!(body["items"].as_array().unwrap().len(), 2);
        assert_eq!(
            body["dashboards"],
            serde_json::json!({
                "a-host": [{"id": "y", "name": "Alpha"}, {"id": "z", "name": "Zulu"}],
                "b-host": []
            })
        );
        // Nothing to list: an empty map.
        let body = get(&fx, Role::Viewer, "/api/v1/problems?host=nope")
            .await
            .json();
        assert_eq!(body["dashboards"], serde_json::json!({}));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn problems_list_with_many_hosts(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        sqlx::query(
            "INSERT INTO hosts (id, name, groups) SELECT gen_random_uuid(), 'h' || lpad(i::text, 3, '0'), ARRAY['g' || (i % 3)] FROM generate_series(1, 300) AS i",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO problems (id, host_id, trigger_name, instance, severity, summary, started_at) SELECT gen_random_uuid(), h.id, 'T', '', 'critical', 's', now() - (substr(h.name, 2)::int || ' minutes')::interval FROM hosts h WHERE h.name LIKE 'h%'",
        )
        .execute(&pool)
        .await
        .unwrap();
        for g in 0..3 {
            add_dashboard(
                &fx,
                &format!("g{g}"),
                &format!(r#"{{"name":"Group {g}","hosts":{{"groups":["g{g}"]}}}}"#),
            )
            .await;
        }
        let body = get(&fx, Role::Viewer, "/api/v1/problems?limit=50")
            .await
            .json();
        assert_eq!(body["total"], 300);
        let on_page: std::collections::BTreeSet<String> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["host"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(on_page.len(), 50);
        let map = body["dashboards"].as_object().unwrap();
        assert_eq!(
            map.keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            on_page
        );
        for (host, list) in map {
            let n: usize = host[1..].parse().unwrap();
            assert_eq!(list.as_array().unwrap().len(), 1, "{host}");
            assert_eq!(list[0]["id"], format!("g{}", n % 3), "{host}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_filters(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        open(&fx, "web-01", "Cpu", Severity::High, T0_MS, false).await;
        open(
            &fx,
            "web-01",
            "Disk 100% full",
            Severity::Warning,
            T0_MS + 1,
            false,
        )
        .await;
        open(&fx, "db-01", "Cpu", Severity::Critical, T0_MS + 2, false).await;
        sqlx::query("UPDATE hosts SET groups = ARRAY['prod'] WHERE name = 'db-01'")
            .execute(&pool)
            .await
            .unwrap();
        for uri in [
            "/api/v1/problems?severity=high",
            "/api/v1/problems?host=db-01&severity=info",
            "/api/v1/problems?group=prod",
        ] {
            let reply = get(&fx, Role::Viewer, uri).await;
            assert_eq!(reply.status, StatusCode::OK, "{uri}");
        }
        let t = |uri: &'static str| {
            let fx = &fx;
            async move { triggers(&get(fx, Role::Viewer, uri).await) }
        };
        assert_eq!(t("/api/v1/problems?severity=high").await, ["Cpu", "Cpu"]);
        assert_eq!(t("/api/v1/problems?host=web-01").await.len(), 2);
        assert_eq!(t("/api/v1/problems?group=prod").await, ["Cpu"]);
        assert_eq!(
            t("/api/v1/problems?search=100%25").await,
            ["Disk 100% full"]
        );
        assert_eq!(t("/api/v1/problems?search=WEB").await.len(), 2);
        // A host name that cannot exist matches nothing, whatever it contains.
        let reply = get(&fx, Role::Viewer, "/api/v1/problems?host=%00").await;
        assert_eq!(
            (reply.status, reply.json()["total"].as_i64()),
            (StatusCode::OK, Some(0))
        );
        let reply = get(&fx, Role::Viewer, "/api/v1/problems?host=Not%20A%20Host").await;
        assert_eq!(
            (reply.status, reply.json()["total"].as_i64()),
            (StatusCode::OK, Some(0))
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn bad_state_or_severity_is_invalid(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        for uri in [
            "/api/v1/problems?state=closed",
            "/api/v1/problems?state=",
            "/api/v1/problems?severity=urgent",
            "/api/v1/problems?severity=HIGH",
            "/api/v1/problems?limit=0",
            "/api/v1/problems?search=a%00b",
            "/api/v1/problems?group=a%0Ab",
        ] {
            let reply = get(&fx, Role::Viewer, uri).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{uri}");
            assert_eq!(reply.error_code(), "invalid", "{uri}");
        }
        let long = "a".repeat(201);
        let reply = get(
            &fx,
            Role::Viewer,
            &format!("/api/v1/problems?search={long}"),
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_dashboard_is_invalid(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        open(&fx, "web", "Cpu", Severity::High, T0_MS, false).await;
        open(&fx, "other", "Cpu", Severity::High, T0_MS, false).await;
        sqlx::query(
            "INSERT INTO config_documents (kind, id, source_file, body) \
             VALUES ('dashboard', 'web-only', 'd.json', \
                     '{\"name\": \"web\", \"hosts\": {\"names\": [\"web\"]}}'::jsonb)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let reply = get(&fx, Role::Viewer, "/api/v1/problems?dashboard=nope").await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(reply.error_code(), "invalid");
        assert_eq!(reply.json()["message"], "no dashboard with this id");
        let reply = get(&fx, Role::Viewer, "/api/v1/problems?dashboard=web-only").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["total"], 1);
        assert_eq!(reply.json()["items"][0]["host"], "web");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn show_one_problem(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let id = open(&fx, "web", "Cpu", Severity::High, T0_MS, false).await;
        let reply = get(&fx, Role::Viewer, &format!("/api/v1/problems/{id}")).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["id"], id.to_string());
        for bad in [Uuid::new_v4().to_string(), "not-an-id".to_owned()] {
            let reply = get(&fx, Role::Viewer, &format!("/api/v1/problems/{bad}")).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{bad}");
            assert_eq!(reply.error_code(), "not_found");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn ack_records_the_session_user(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let id = open(&fx, "web", "Cpu", Severity::High, T0_MS, false).await;
        let uri = format!("/api/v1/problems/{id}/ack");
        let reply = post(
            &fx,
            Role::Operator,
            &uri,
            r#"{"comment": "  looking\tinto it\n"}"#,
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        let p = reply.json();
        assert_eq!(p["acknowledged_by"], "operator");
        assert_eq!(p["ack_comment"], "looking into it");
        assert!(p["acknowledged_at"].is_string());
        assert!(p["resolved_at"].is_null());

        // Again, by an admin, with no comment: it overwrites.
        let reply = post(&fx, Role::Admin, &uri, "{}").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["acknowledged_by"], "admin");
        assert_eq!(reply.json()["ack_comment"], "");

        // Over 1,000 characters are cut.
        let long = "x".repeat(1500);
        let reply = post(
            &fx,
            Role::Operator,
            &uri,
            &format!(r#"{{"comment": "{long}"}}"#),
        )
        .await;
        assert_eq!(reply.json()["ack_comment"].as_str().unwrap().len(), 1000);

        // A malformed id and an unknown one are 404; an unknown field is refused.
        let reply = post(&fx, Role::Operator, "/api/v1/problems/nope/ack", "{}").await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
        let unknown = format!("/api/v1/problems/{}/ack", Uuid::new_v4());
        let reply = post(&fx, Role::Operator, &unknown, "{}").await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
        let reply = post(&fx, Role::Operator, &uri, r#"{"by": "someone"}"#).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn ack_by_viewer_is_forbidden(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let id = open(&fx, "web", "Cpu", Severity::High, T0_MS, false).await;
        for action in ["ack", "close"] {
            let uri = format!("/api/v1/problems/{id}/{action}");
            let reply = post(&fx, Role::Viewer, &uri, r#"{"comment": "x"}"#).await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN, "{action}");
            assert_eq!(reply.error_code(), "forbidden");
        }
        let p = fx.store.problem(id).await.unwrap().unwrap();
        assert!(p.acknowledged_at.is_none() && p.resolved_at.is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn ack_of_resolved_is_conflict(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let id = open(&fx, "web", "Cpu", Severity::High, T0_MS, false).await;
        let close = format!("/api/v1/problems/{id}/close");
        let ack = format!("/api/v1/problems/{id}/ack");
        assert_eq!(
            post(&fx, Role::Operator, &close, r#"{"comment": "gone"}"#)
                .await
                .status,
            StatusCode::OK
        );
        let reply = post(&fx, Role::Operator, &ack, "{}").await;
        assert_eq!(reply.status, StatusCode::CONFLICT);
        assert_eq!(reply.error_code(), "conflict");
        // Closing twice is a conflict too.
        let reply = post(&fx, Role::Operator, &close, r#"{"comment": "again"}"#).await;
        assert_eq!(reply.status, StatusCode::CONFLICT);
        assert_eq!(reply.error_code(), "conflict");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn close_needs_a_comment(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let id = open(&fx, "web", "Cpu", Severity::High, T0_MS, false).await;
        let uri = format!("/api/v1/problems/{id}/close");
        for body in [
            "{}",
            r#"{"comment": ""}"#,
            r#"{"comment": "   "}"#,
            r#"{"comment": "\n\t "}"#,
            r#"{"comment": null}"#,
        ] {
            let reply = post(&fx, Role::Operator, &uri, body).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(reply.error_code(), "invalid", "{body}");
        }
        assert!(
            fx.store
                .problem(id)
                .await
                .unwrap()
                .unwrap()
                .resolved_at
                .is_none()
        );
        let reply = post(&fx, Role::Operator, &uri, r#"{"comment": "unmounted"}"#).await;
        assert_eq!(reply.status, StatusCode::OK);
        let p = reply.json();
        assert_eq!(p["resolve_reason"], "closed_by_operator");
        assert_eq!(p["closed_by"], "operator");
        assert_eq!(p["close_comment"], "unmounted");
        assert!(p["acknowledged_at"].is_null() && p["acknowledged_by"].is_null());
        assert!(p["resolved_at"].is_string());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn closing_an_acknowledged_problem_records_the_closer(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let id = open(&fx, "web", "Cpu", Severity::High, T0_MS, false).await;
        let reply = post(
            &fx,
            Role::Admin,
            &format!("/api/v1/problems/{id}/ack"),
            r#"{"comment": "a"}"#,
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        let reply = post(
            &fx,
            Role::Operator,
            &format!("/api/v1/problems/{id}/close"),
            r#"{"comment": "b"}"#,
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        let p = reply.json();
        assert_eq!(p["acknowledged_by"], "admin");
        assert_eq!(p["ack_comment"], "a");
        assert_eq!(p["closed_by"], "operator");
        assert_eq!(p["close_comment"], "b");
        // The stored problem shows the same.
        let shown = get(&fx, Role::Viewer, &format!("/api/v1/problems/{id}")).await;
        assert_eq!(shown.json(), p);
    }

    /// Closes a problem whose start was mailed; whether the end is queued follows
    /// `queue_mail` of the server.
    async fn close_with_mail_setting(pool: PgPool, queue_mail: bool) {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        let compiled = compile(&read_tree(&dir, "c1").unwrap()).unwrap();
        let fx = Fixture::with_options(pool.clone(), config(true, None), queue_mail).await;
        fx.store
            .apply_config("c1", &compiled, time::OffsetDateTime::now_utc(), 100, false)
            .await
            .unwrap();
        // web-01 is in the group the example rule mails: the start is queued.
        let id = open(&fx, "web-01", "Disk", Severity::High, T0_MS, true).await;
        let count = |event: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM notification_outbox \
                     WHERE problem_id = $1 AND event = $2",
                )
                .bind(id)
                .bind(event)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(count("started").await, 1, "the start is queued");
        let uri = format!("/api/v1/problems/{id}/close");
        let reply = post(&fx, Role::Operator, &uri, r#"{"comment": "gone"}"#).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(count("resolved").await, i64::from(queue_mail));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn close_resolves_and_queues_mail_when_configured(pool: PgPool) {
        close_with_mail_setting(pool, true).await;
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn close_queues_no_mail_when_not_configured(pool: PgPool) {
        close_with_mail_setting(pool, false).await;
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn closed_problem_reopens_on_the_next_firing_value(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let id = open(&fx, "web", "Cpu", Severity::High, T0_MS, false).await;
        // The state of the firing trigger, as the evaluation keeps it.
        let host: Uuid = sqlx::query_scalar("SELECT host_id FROM problems WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO trigger_states (host_id, trigger_name, instance, firing, \
             condition_since, last_ts, last_value) VALUES ($1, 'Cpu', '', true, now(), now(), 95)",
        )
        .bind(host)
        .execute(&pool)
        .await
        .unwrap();
        let uri = format!("/api/v1/problems/{id}/close");
        let reply = post(&fx, Role::Operator, &uri, r#"{"comment": "gone"}"#).await;
        assert_eq!(reply.status, StatusCode::OK);
        let states: i64 = sqlx::query_scalar("SELECT count(*) FROM trigger_states")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(states, 0, "the trigger state is cleaned");
        assert!(triggers(&get(&fx, Role::Viewer, "/api/v1/problems").await).is_empty());

        // The next value that fires opens a new problem.
        let again = open(&fx, "web", "Cpu", Severity::High, T0_MS + 10_000, false).await;
        assert_ne!(again, id);
        let reply = get(&fx, Role::Viewer, "/api/v1/problems").await;
        assert_eq!(triggers(&reply), ["Cpu"]);
        assert_eq!(reply.json()["items"][0]["id"], again.to_string());
        let reply = get(&fx, Role::Viewer, "/api/v1/problems?state=all").await;
        assert_eq!(reply.json()["total"], 2);
    }
}
