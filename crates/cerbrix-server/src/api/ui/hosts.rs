//! The hosts list, the groups and the detail of one host.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use cerbrix_common::config::Severity;
use cerbrix_common::validate_host_name;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{ApiError, Entry, Listed, Page, invalid};
use crate::api::AppState;
use crate::auth::Viewer;
use crate::gitops::{EffectiveHost, EffectiveItem, EffectiveTrigger};
use crate::state::{HostDetail, HostQuery, HostStateFilter, Problem, StateError};
use crate::view::{HostState, host_state, serialize_time_opt};

pub(super) fn entries() -> Vec<Entry> {
    vec![
        Entry::new("GET", "/api/v1/hosts", list),
        Entry::new("GET", "/api/v1/host-groups", groups),
        Entry::new("GET", "/api/v1/hosts/{name}", detail),
    ]
}

/// The longest search text taken.
const MAX_SEARCH_CHARS: usize = 200;

fn store_failed(what: &str, e: &StateError) -> ApiError {
    tracing::error!(error = %e, "{what}");
    ApiError::internal()
}

/// A host of the list.
#[derive(Serialize)]
struct HostOut {
    name: String,
    address: Option<String>,
    groups: Vec<String>,
    state: Option<HostState>,
    #[serde(serialize_with = "serialize_time_opt")]
    last_seen_at: Option<OffsetDateTime>,
    managed_by_git: bool,
}

#[derive(Deserialize)]
struct HostsParams {
    search: Option<String>,
    group: Option<String>,
    state: Option<String>,
}

async fn list(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    page: Page,
    params: Result<Query<HostsParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Listed<HostOut>>, ApiError> {
    let Query(params) = params.map_err(|_| invalid("the query parameters are malformed"))?;
    if params
        .search
        .as_deref()
        .is_some_and(|s| s.chars().count() > MAX_SEARCH_CHARS)
    {
        return Err(invalid("the search text must have at most 200 characters"));
    }
    let state = params
        .state
        .as_deref()
        .map(|s| {
            s.parse::<HostStateFilter>()
                .map_err(|()| invalid("state must be problems, no_data or ok"))
        })
        .transpose()?;
    let query = HostQuery {
        search: params.search,
        group: params.group,
        state,
        limit: page.limit,
        offset: page.offset,
    };
    let (rows, total) = app
        .store
        .hosts_page(&query)
        .await
        .map_err(|e| store_failed("listing hosts failed", &e))?;
    let managed_by_git = app.config.gitops.enabled;
    Ok(Json(Listed {
        items: rows
            .into_iter()
            .map(|row| HostOut {
                state: host_state(&row.open),
                name: row.name,
                address: row.address,
                groups: row.groups,
                last_seen_at: row.last_seen_at,
                managed_by_git,
            })
            .collect(),
        total,
    }))
}

#[derive(Serialize)]
struct Groups {
    items: Vec<String>,
}

async fn groups(State(app): State<AppState>, Viewer(_): Viewer) -> Result<Json<Groups>, ApiError> {
    let items = app
        .store
        .host_groups()
        .await
        .map_err(|e| store_failed("listing host groups failed", &e))?;
    Ok(Json(Groups { items }))
}

#[derive(Serialize)]
struct TriggerOut {
    name: String,
    expression: String,
    severity: Severity,
    origin: String,
    firing: bool,
}

#[derive(Serialize)]
struct DetailOut {
    name: String,
    address: Option<String>,
    groups: Vec<String>,
    templates: Vec<String>,
    source_file: Option<String>,
    managed_by_git: bool,
    #[serde(serialize_with = "serialize_time_opt")]
    enrolled_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "serialize_time_opt")]
    last_seen_at: Option<OffsetDateTime>,
    state: Option<HostState>,
    items: Vec<EffectiveItem>,
    triggers: Vec<TriggerOut>,
    open_problems: Vec<Problem>,
}

async fn detail(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    Path(name): Path<String>,
) -> Result<Json<DetailOut>, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such host");
    // A name that cannot be a host name names no host.
    if validate_host_name(&name).is_err() {
        return Err(not_found());
    }
    let HostDetail {
        row,
        config,
        enrolled_at,
        firing,
    } = app
        .store
        .host_detail(&name)
        .await
        .map_err(|e| store_failed("reading a host failed", &e))?
        .ok_or_else(not_found)?;
    // A host created by hand has no stored configuration: nothing is configured.
    let config = config.unwrap_or(EffectiveHost {
        templates: Vec::new(),
        items: Vec::new(),
        triggers: Vec::new(),
    });
    Ok(Json(DetailOut {
        state: host_state(&row.open),
        name: row.name,
        address: row.address,
        groups: row.groups,
        templates: config.templates,
        source_file: row.source_file,
        managed_by_git: app.config.gitops.enabled,
        enrolled_at,
        last_seen_at: row.last_seen_at,
        items: config.items,
        triggers: config
            .triggers
            .into_iter()
            .map(|t: EffectiveTrigger| TriggerOut {
                firing: firing.contains(&t.name),
                name: t.name,
                expression: t.expression,
                severity: t.severity,
                origin: t.origin,
            })
            .collect(),
        open_problems: row.open,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use cerbrix_common::MetricValue;
    use cerbrix_common::config::Severity;
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use time::OffsetDateTime;

    use crate::alerting::{
        ActiveTrigger, Evaluation, OpenProblem, ProblemKind, active_triggers, eval::evaluate,
    };
    use crate::api::ui::testing::*;
    use crate::auth::Role;

    const T0_MS: i64 = 1_800_000_000_000;

    /// Loads the example repository: `web-01` and `db-01`.
    async fn apply_example(fx: &Fixture) {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        let compiled =
            crate::gitops::compile(&crate::gitops::read_tree(&dir, "c1").unwrap()).unwrap();
        fx.store
            .apply_config(
                "c1",
                &compiled,
                OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
                50,
                false,
            )
            .await
            .unwrap();
    }

    async fn get(fx: &Fixture, uri: &str) -> Reply {
        let token = fx.session(Role::Viewer).await;
        fx.call("GET", uri, Some(&token), &[], None).await
    }

    async fn open_problem(fx: &Fixture, host: &str, trigger: &str, severity: Severity) {
        let id = fx.store.host_by_name(host).await.unwrap().unwrap().id;
        let problem = OpenProblem {
            trigger: trigger.to_owned(),
            instance: String::new(),
            severity,
            at_ms: T0_MS,
            summary: format!("{trigger} is bad"),
            kind: ProblemKind::Compare,
        };
        fx.store
            .evaluate_host(id, false, |_| Evaluation {
                open: vec![problem],
                ..Evaluation::default()
            })
            .await
            .unwrap();
    }

    fn keys(value: &Value) -> Vec<&str> {
        let mut keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn hosts_list_shape(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        apply_example(&fx).await;
        fx.store.create_host("zz-bare").await.unwrap();
        open_problem(&fx, "web-01", "Disk almost full", Severity::High).await;

        let reply = get(&fx, "/api/v1/hosts").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(keys(&body), ["items", "total"]);
        assert_eq!(body["total"], 3);
        let items = body["items"].as_array().unwrap();
        let names: Vec<_> = items.iter().map(|h| h["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["web-01", "db-01", "zz-bare"]);

        let web = &items[0];
        assert_eq!(
            keys(web),
            [
                "address",
                "groups",
                "last_seen_at",
                "managed_by_git",
                "name",
                "state"
            ]
        );
        assert_eq!(web["address"], "10.0.4.12");
        assert_eq!(web["groups"], json!(["production", "web"]));
        assert_eq!(web["managed_by_git"], false);
        assert!(web["last_seen_at"].is_null());
        let state = &web["state"];
        assert_eq!(
            keys(state),
            [
                "acknowledged",
                "instance",
                "kind",
                "open_problems",
                "severity",
                "summary",
                "trigger"
            ]
        );
        assert_eq!(state["severity"], "high");
        assert_eq!(state["acknowledged"], false);
        assert_eq!(state["kind"], "compare");
        assert_eq!(state["trigger"], "Disk almost full");
        assert_eq!(state["open_problems"], 1);
        assert!(items[1]["state"].is_null());
        assert!(items[2]["address"].is_null());
        assert_eq!(items[2]["groups"], json!([]));

        // Filters and paging reach the store.
        let reply = get(&fx, "/api/v1/hosts?search=DB&limit=1").await;
        assert_eq!(reply.json()["total"], 1);
        assert_eq!(reply.json()["items"][0]["name"], "db-01");
        let reply = get(&fx, "/api/v1/hosts?group=web").await;
        assert_eq!(reply.json()["total"], 1);
        let reply = get(&fx, "/api/v1/hosts?state=problems").await;
        assert_eq!(reply.json()["items"].as_array().unwrap().len(), 1);
        let reply = get(&fx, "/api/v1/hosts?state=ok&limit=1&offset=1").await;
        assert_eq!(reply.json()["total"], 2);
        assert_eq!(reply.json()["items"][0]["name"], "zz-bare");

        let reply = get(&fx, "/api/v1/host-groups").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json(), json!({"items": ["db", "production", "web"]}));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_detail_shape(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        apply_example(&fx).await;
        let host = fx.store.host_by_name("web-01").await.unwrap().unwrap();
        fx.store.bind_credential(host.id).await.unwrap();

        // A value that makes `Disk almost full` (> 90 for 10m) fire: held for 11 minutes.
        for (ts, value) in [(T0_MS, 95.0), (T0_MS + 11 * 60_000, 96.0)] {
            fx.store
                .evaluate_host(host.id, false, |ctx| {
                    let triggers: Vec<ActiveTrigger> =
                        active_triggers(ctx.config.as_ref().unwrap());
                    evaluate(
                        &triggers,
                        &ctx.states,
                        &[MetricValue {
                            key: "disk.used_percent".into(),
                            params: [("mount".to_owned(), "/".to_owned())].into(),
                            ts_ms: ts,
                            value,
                        }],
                        ts,
                        60_000,
                    )
                })
                .await
                .unwrap();
        }

        let reply = get(&fx, "/api/v1/hosts/web-01").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(
            keys(&body),
            [
                "address",
                "enrolled_at",
                "groups",
                "items",
                "last_seen_at",
                "managed_by_git",
                "name",
                "open_problems",
                "source_file",
                "state",
                "templates",
                "triggers"
            ]
        );
        assert_eq!(body["name"], "web-01");
        assert_eq!(body["address"], "10.0.4.12");
        assert_eq!(body["groups"], json!(["production", "web"]));
        assert_eq!(body["templates"], json!(["linux-base"]));
        assert_eq!(body["source_file"], "hosts/web-01.json");
        assert_eq!(body["managed_by_git"], false);
        let enrolled = body["enrolled_at"].as_str().unwrap();
        assert!(enrolled.ends_with('Z'), "{enrolled}");

        let items = body["items"].as_array().unwrap();
        let cpu = items.iter().find(|i| i["key"] == "cpu.usage").unwrap();
        assert_eq!(keys(cpu), ["interval_secs", "key", "origin", "params"]);
        assert_eq!(cpu["interval_secs"], 10);
        assert_eq!(cpu["origin"], "host");
        assert!(cpu["params"].is_null());
        let memory = items
            .iter()
            .find(|i| i["key"] == "memory.used_percent")
            .unwrap();
        assert_eq!(memory["origin"], "template:linux-base");

        let triggers = body["triggers"].as_array().unwrap();
        let disk = triggers
            .iter()
            .find(|t| t["name"] == "Disk almost full")
            .unwrap();
        assert_eq!(
            keys(disk),
            ["expression", "firing", "name", "origin", "severity"]
        );
        assert_eq!(disk["firing"], true);
        assert_eq!(disk["severity"], "high");
        assert_eq!(disk["origin"], "template:linux-base");
        assert_eq!(disk["expression"], "disk.used_percent > 90 for 10m");
        let silent = triggers
            .iter()
            .find(|t| t["name"] == "Agent silent")
            .unwrap();
        assert_eq!(silent["firing"], false);

        let problems = body["open_problems"].as_array().unwrap();
        assert_eq!(problems.len(), 1);
        let p = &problems[0];
        assert_eq!(
            keys(p),
            [
                "ack_comment",
                "acknowledged_at",
                "acknowledged_by",
                "host",
                "id",
                "instance",
                "kind",
                "resolve_reason",
                "resolved_at",
                "severity",
                "started_at",
                "summary",
                "trigger"
            ]
        );
        assert_eq!(p["host"], "web-01");
        assert_eq!(p["trigger"], "Disk almost full");
        assert!(p["resolved_at"].is_null());
        assert_eq!(body["state"]["trigger"], "Disk almost full");
        assert_eq!(body["state"]["open_problems"], 1);

        // No credential material leaves the server.
        let text = String::from_utf8_lossy(&reply.body).to_lowercase();
        assert!(
            !text.contains("credential") && !text.contains("hash"),
            "{text}"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_without_configuration_has_empty_lists(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let reply = get(&fx, "/api/v1/hosts/fixture-host").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        for key in ["templates", "items", "triggers", "open_problems", "groups"] {
            assert_eq!(body[key], json!([]), "{key}");
        }
        for key in [
            "address",
            "source_file",
            "enrolled_at",
            "last_seen_at",
            "state",
        ] {
            assert!(body[key].is_null(), "{key}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_host_is_not_found(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        for name in ["nope", "bad%20name", "..", "-x", "a%2Fb"] {
            let reply = get(&fx, &format!("/api/v1/hosts/{name}")).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{name}");
            assert_eq!(reply.error_code(), "not_found", "{name}");
        }
        let long = "a".repeat(254);
        let reply = get(&fx, &format!("/api/v1/hosts/{long}")).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);

        let host = fx
            .store
            .host_by_name("fixture-host")
            .await
            .unwrap()
            .unwrap();
        fx.store.set_host_active(host.id, false).await.unwrap();
        let reply = get(&fx, "/api/v1/hosts/fixture-host").await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn bad_query_parameters_are_invalid(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let long = "a".repeat(201);
        for uri in [
            "/api/v1/hosts?state=bogus".to_owned(),
            "/api/v1/hosts?state=".to_owned(),
            "/api/v1/hosts?state=OK".to_owned(),
            "/api/v1/hosts?limit=0".to_owned(),
            "/api/v1/hosts?offset=-1".to_owned(),
            format!("/api/v1/hosts?search={long}"),
        ] {
            let reply = get(&fx, &uri).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{uri}");
            assert_eq!(reply.error_code(), "invalid", "{uri}");
        }
        let exact = "a".repeat(200);
        let reply = get(&fx, &format!("/api/v1/hosts?search={exact}")).await;
        assert_eq!(reply.status, StatusCode::OK);
        for state in ["problems", "no_data", "ok"] {
            let reply = get(&fx, &format!("/api/v1/hosts?state={state}")).await;
            assert_eq!(reply.status, StatusCode::OK, "{state}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn managed_by_git_follows_the_setting(pool: PgPool) {
        let mut config = (*config(true, None)).clone();
        config.gitops.enabled = true;
        let fx = Fixture::with_config(pool, Arc::new(config)).await;
        let list = get(&fx, "/api/v1/hosts").await.json();
        assert_eq!(list["items"][0]["managed_by_git"], true);
        let detail = get(&fx, "/api/v1/hosts/fixture-host").await.json();
        assert_eq!(detail["managed_by_git"], true);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_search_is_literal_over_http(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.store.create_host("a_b").await.unwrap();
        fx.store.create_host("axb").await.unwrap();
        let reply = get(&fx, "/api/v1/hosts?search=a_b").await;
        assert_eq!(reply.json()["total"], 1);
        let reply = get(&fx, "/api/v1/hosts?search=%25").await;
        assert_eq!(reply.json()["total"], 0);
    }
}
