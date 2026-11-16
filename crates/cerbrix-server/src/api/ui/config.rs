//! The routes behind the page header and the Configuration view: overall status, sync
//! history, unknown agents, enrolment keys and releasing a host's agent binding.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use cerbrix_common::validate_host_name;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::{ApiError, ApiJson, Entry, Listed, Page, invalid, query_text};
use crate::admin::parse_duration;
use crate::api::AppState;
use crate::auth::{Admin, Operator, Viewer};
use crate::gitops::SyncIssue;
use crate::state::{
    EnrollmentKey, HostCounts, ProblemCounts, StateError, SyncChanges, SyncHistoryEntry,
    UnknownAgent,
};

/// Most unknown agents the list shows.
const MAX_UNKNOWN_AGENTS: i64 = 500;

pub(super) fn entries() -> Vec<Entry> {
    vec![
        Entry::new("GET", "/api/v1/status", status),
        Entry::new("GET", "/api/v1/sync/history", sync_history),
        Entry::new("GET", "/api/v1/unknown-agents", unknown_agents),
        Entry::new("GET", "/api/v1/enrollment-keys", list_keys),
        Entry::new("POST", "/api/v1/enrollment-keys", create_key),
        Entry::new("DELETE", "/api/v1/enrollment-keys/{id}", revoke_key),
        Entry::new("POST", "/api/v1/hosts/{name}/release", release),
    ]
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn store_failed(what: &str, e: &StateError) -> ApiError {
    tracing::error!(error = %e, "{what}");
    ApiError::internal()
}

#[derive(Serialize)]
struct SyncOut {
    status: &'static str,
    active_commit: Option<String>,
    last_success_at: Option<String>,
    last_attempt_at: Option<String>,
    /// As stored; data, never HTML.
    issues: Vec<SyncIssue>,
}

#[derive(Serialize)]
struct StatusOut {
    version: &'static str,
    gitops: bool,
    mail: bool,
    sync: SyncOut,
    hosts: HostCounts,
    problems: ProblemCounts,
}

/// What the page header needs on every refresh: three queries, run together.
async fn status(
    State(app): State<AppState>,
    Viewer(_): Viewer,
) -> Result<Json<StatusOut>, ApiError> {
    let (sync, hosts, problems) = tokio::try_join!(
        app.store.sync_state(),
        app.store.host_counts(),
        app.store.open_problem_counts(),
    )
    .map_err(|e| store_failed("reading the status failed", &e))?;
    Ok(Json(StatusOut {
        version: env!("CARGO_PKG_VERSION"),
        gitops: app.config.gitops.enabled,
        mail: app.queue_mail,
        sync: SyncOut {
            status: sync.status.as_str(),
            active_commit: sync.active_commit,
            last_success_at: sync.last_success_at.map(rfc3339),
            last_attempt_at: sync.last_attempt_at.map(rfc3339),
            issues: sync.issues,
        },
        hosts,
        problems,
    }))
}

#[derive(Serialize)]
struct SyncHistoryOut {
    commit: String,
    applied_at: String,
    changes: SyncChanges,
}

async fn sync_history(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    page: Page,
) -> Result<Json<Listed<SyncHistoryOut>>, ApiError> {
    let (entries, total) = app
        .store
        .sync_history_page(page.limit, page.offset)
        .await
        .map_err(|e| store_failed("reading the sync history failed", &e))?;
    let items = entries
        .into_iter()
        .map(
            |SyncHistoryEntry {
                 commit,
                 applied_at,
                 changes,
             }| SyncHistoryOut {
                commit,
                applied_at: rfc3339(applied_at),
                changes,
            },
        )
        .collect();
    Ok(Json(Listed { items, total }))
}

#[derive(Serialize)]
struct UnknownAgentOut {
    host_name: String,
    first_seen: String,
    last_seen: String,
    attempts: i64,
}

async fn unknown_agents(
    State(app): State<AppState>,
    Operator(_): Operator,
) -> Result<Json<serde_json::Value>, ApiError> {
    let agents = app
        .store
        .unknown_agents_recent(MAX_UNKNOWN_AGENTS)
        .await
        .map_err(|e| store_failed("reading the unknown agents failed", &e))?;
    let items: Vec<UnknownAgentOut> = agents
        .into_iter()
        .map(
            |UnknownAgent {
                 host_name,
                 first_seen,
                 last_seen,
                 attempts,
             }| UnknownAgentOut {
                host_name,
                first_seen: rfc3339(first_seen),
                last_seen: rfc3339(last_seen),
                attempts,
            },
        )
        .collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

/// An enrolment key as the API shows it: never the key or its hash. `key` is set only in
/// the answer to creating it.
#[derive(Serialize)]
struct KeyOut {
    id: String,
    description: String,
    created_at: String,
    expires_at: String,
    revoked_at: Option<String>,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

impl KeyOut {
    fn new(key: &EnrollmentKey, now: OffsetDateTime, plain: Option<String>) -> Self {
        let state = if key.revoked_at.is_some() {
            "revoked"
        } else if key.expires_at <= now {
            "expired"
        } else {
            "active"
        };
        Self {
            id: key.id.to_string(),
            description: key.description.clone(),
            created_at: rfc3339(key.created_at),
            expires_at: rfc3339(key.expires_at),
            revoked_at: key.revoked_at.map(rfc3339),
            state,
            key: plain,
        }
    }
}

async fn list_keys(
    State(app): State<AppState>,
    Admin(_): Admin,
) -> Result<Json<serde_json::Value>, ApiError> {
    let now = OffsetDateTime::now_utc();
    let keys = app
        .store
        .enrollment_keys()
        .await
        .map_err(|e| store_failed("listing enrolment keys failed", &e))?;
    let items: Vec<KeyOut> = keys.iter().map(|k| KeyOut::new(k, now, None)).collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateKeyBody {
    description: String,
    expires_in: String,
}

async fn create_key(
    State(app): State<AppState>,
    Admin(admin): Admin,
    ApiJson(body): ApiJson<CreateKeyBody>,
) -> Result<(StatusCode, Json<KeyOut>), ApiError> {
    let description = query_text(
        &body.description,
        200,
        "the description must have at most 200 characters and no control character",
    )?;
    if description.is_empty() {
        return Err(invalid("the description must not be empty"));
    }
    const BAD_EXPIRY: &str = "expires_in must be between 1h and 365d, e.g. 30d, 12h or 90m";
    let lifetime = parse_duration(&body.expires_in).map_err(|_| invalid(BAD_EXPIRY))?;
    if !(Duration::hours(1)..=Duration::days(365)).contains(&lifetime) {
        return Err(invalid(BAD_EXPIRY));
    }
    let now = OffsetDateTime::now_utc();
    let (id, plain) = app
        .store
        .create_enrollment_key(description, now + lifetime)
        .await
        .map_err(|e| store_failed("creating an enrolment key failed", &e))?;
    let key = app
        .store
        .enrollment_key(id)
        .await
        .map_err(|e| store_failed("reading an enrolment key failed", &e))?
        .ok_or_else(ApiError::internal)?;
    tracing::info!(admin = %admin.name, key_id = %id, description, "enrolment key created");
    Ok((
        StatusCode::CREATED,
        Json(KeyOut::new(&key, now, Some(plain))),
    ))
}

async fn revoke_key(
    State(app): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such enrolment key");
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    app.store
        .revoke_enrollment_key(id)
        .await
        .map_err(|e| match e {
            StateError::NotFound => not_found(),
            e => store_failed("revoking an enrolment key failed", &e),
        })?;
    tracing::info!(admin = %admin.name, key_id = %id, "enrolment key revoked");
    Ok(StatusCode::NO_CONTENT)
}

/// Removes the agent binding of an active host so it can enrol again. Allowed with GitOps
/// on: it changes no configuration.
async fn release(
    State(app): State<AppState>,
    Admin(admin): Admin,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such host");
    // A name that cannot be a host name names no host.
    if validate_host_name(&name).is_err() {
        return Err(not_found());
    }
    let host = app
        .store
        .host_by_name(&name)
        .await
        .map_err(|e| store_failed("reading a host failed", &e))?
        .filter(|h| h.active)
        .ok_or_else(not_found)?;
    app.store
        .release_binding(host.id)
        .await
        .map_err(|e| store_failed("releasing a host failed", &e))?;
    tracing::info!(admin = %admin.name, host = %host.name, "agent binding released");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use crate::api::ui::testing::*;
    use crate::auth::Role;
    use crate::gitops::SyncIssue;

    async fn get(fx: &Fixture, role: Role, uri: &str) -> Reply {
        let token = fx.session(role).await;
        fx.call("GET", uri, Some(&token), &[], None).await
    }

    async fn create_key(fx: &Fixture, body: Value) -> Reply {
        let token = fx.session(Role::Admin).await;
        fx.call(
            "POST",
            "/api/v1/enrollment-keys",
            Some(&token),
            &[],
            Some(&body.to_string()),
        )
        .await
    }

    async fn insert_problem(fx: &Fixture, host: &str, severity: &str, acked: bool) {
        let host = fx.store.host_by_name(host).await.unwrap().unwrap().id;
        sqlx::query(
            "INSERT INTO problems (id, host_id, trigger_name, instance, severity, summary, \
             started_at, acknowledged_at) VALUES ($1, $2, $3, '', $4, 's', now(), \
             CASE WHEN $5 THEN now() END)",
        )
        .bind(Uuid::new_v4())
        .bind(host)
        .bind(Uuid::new_v4().to_string())
        .bind(severity)
        .bind(acked)
        .execute(&fx.pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn status_shape_in_sync_and_out_of_sync(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let reply = get(&fx, Role::Viewer, "/api/v1/status").await;
        assert_eq!(reply.status, StatusCode::OK);
        let v = reply.json();
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["sync"]["status"], "never");
        assert_eq!(v["sync"]["active_commit"], Value::Null);
        assert_eq!(v["sync"]["issues"], json!([]));

        let now = OffsetDateTime::now_utc();
        sqlx::query(
            "UPDATE sync_state SET status = 'in_sync', active_commit = 'abc123', \
             last_success_at = $1, last_attempt_at = $1",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let v = get(&fx, Role::Viewer, "/api/v1/status").await.json();
        assert_eq!(v["sync"]["status"], "in_sync");
        assert_eq!(v["sync"]["active_commit"], "abc123");
        assert!(
            v["sync"]["last_success_at"]
                .as_str()
                .is_some_and(|t| t.ends_with('Z'))
        );

        let issues = [
            SyncIssue {
                file: Some("hosts/web-01.json".into()),
                line: Some(3),
                column: Some(7),
                reason: "<b>unknown template</b>".into(),
            },
            SyncIssue {
                file: None,
                line: None,
                column: None,
                reason: "repository unreachable".into(),
            },
        ];
        fx.store
            .record_sync_failure(Some("def456"), &issues, now)
            .await
            .unwrap();
        let v = get(&fx, Role::Viewer, "/api/v1/status").await.json();
        assert_eq!(v["sync"]["status"], "out_of_sync");
        // The active commit stays the last good one.
        assert_eq!(v["sync"]["active_commit"], "abc123");
        assert_eq!(
            v["sync"]["issues"],
            json!([
                {"file": "hosts/web-01.json", "line": 3, "column": 7,
                 "reason": "<b>unknown template</b>"},
                {"file": null, "line": null, "column": null,
                 "reason": "repository unreachable"},
            ])
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn status_counts(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let seen = fx.store.create_host("seen").await.unwrap();
        fx.store.create_host("never-seen").await.unwrap();
        let gone = fx.store.create_host("gone").await.unwrap();
        sqlx::query("UPDATE hosts SET last_seen_at = now() WHERE id = $1")
            .bind(seen.id)
            .execute(&pool)
            .await
            .unwrap();
        // An open problem of an inactive host does not count for the hosts.
        insert_problem(&fx, "gone", "info", false).await;
        fx.store.set_host_active(gone.id, false).await.unwrap();
        // Two problems on one host count the host once.
        insert_problem(&fx, FIXTURE_HOST, "critical", false).await;
        insert_problem(&fx, FIXTURE_HOST, "critical", true).await;
        insert_problem(&fx, "seen", "high", true).await;
        // A resolved problem counts nowhere.
        insert_problem(&fx, "seen", "warning", false).await;
        sqlx::query("UPDATE problems SET resolved_at = now() WHERE severity = 'warning'")
            .execute(&pool)
            .await
            .unwrap();

        let v = get(&fx, Role::Viewer, "/api/v1/status").await.json();
        assert_eq!(
            v["hosts"],
            json!({"total": 3, "with_problems": 2, "without_problems": 1, "never_seen": 2})
        );
        assert_eq!(
            v["problems"]["critical"],
            json!({"open": 2, "unacknowledged": 1})
        );
        assert_eq!(
            v["problems"]["high"],
            json!({"open": 1, "unacknowledged": 0})
        );
        assert_eq!(
            v["problems"]["warning"],
            json!({"open": 0, "unacknowledged": 0})
        );
        assert_eq!(
            v["problems"]["info"],
            json!({"open": 1, "unacknowledged": 1})
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn status_flags_are_off_by_default(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let v = get(&fx, Role::Viewer, "/api/v1/status").await.json();
        assert_eq!(
            (v["gitops"].clone(), v["mail"].clone()),
            (json!(false), json!(false))
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn status_reports_gitops_and_mail_flags(pool: PgPool) {
        let mut config = (*config(true, None)).clone();
        config.gitops.enabled = true;
        let fx = Fixture::with_options(pool, Arc::new(config), true).await;
        let v = get(&fx, Role::Viewer, "/api/v1/status").await.json();
        assert_eq!(
            (v["gitops"].clone(), v["mail"].clone()),
            (json!(true), json!(true))
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn sync_history_pages(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let empty = get(&fx, Role::Viewer, "/api/v1/sync/history").await.json();
        assert_eq!(empty, json!({"items": [], "total": 0}));

        let t0 = OffsetDateTime::now_utc() - Duration::hours(10);
        for n in 0..5 {
            let changes = json!({
                "hosts": {"added": [format!("h{n}")], "changed": [], "removed": []},
                "templates": {"added": [], "changed": [], "removed": []},
                "notifications": {"added": [], "changed": [], "removed": []},
                "dashboards": {"added": [], "changed": [], "removed": []},
                "displays": {"added": [], "changed": [], "removed": []},
            });
            sqlx::query(
                "INSERT INTO sync_history (commit, applied_at, changes) VALUES ($1, $2, $3)",
            )
            .bind(format!("c{n}"))
            .bind(t0 + Duration::hours(n))
            .bind(changes)
            .execute(&pool)
            .await
            .unwrap();
        }
        let v = get(&fx, Role::Viewer, "/api/v1/sync/history?limit=2&offset=1")
            .await
            .json();
        assert_eq!(v["total"], 5);
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        // Newest first: c4, c3, c2, ...
        assert_eq!(items[0]["commit"], "c3");
        assert_eq!(items[1]["commit"], "c2");
        assert_eq!(items[0]["changes"]["hosts"]["added"], json!(["h3"]));
        assert_eq!(items[0]["changes"]["displays"]["removed"], json!([]));
        assert!(items[0]["applied_at"].as_str().unwrap().ends_with('Z'));
        let all = get(&fx, Role::Viewer, "/api/v1/sync/history").await.json();
        assert_eq!(all["items"][0]["commit"], "c4");
        let bad = get(&fx, Role::Viewer, "/api/v1/sync/history?limit=0").await;
        assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_agents_listed_for_operator_not_viewer(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let t = OffsetDateTime::now_utc();
        fx.store
            .record_unknown_agent("older", t - Duration::hours(2))
            .await
            .unwrap();
        fx.store
            .record_unknown_agent("newer", t - Duration::hours(1))
            .await
            .unwrap();
        fx.store.record_unknown_agent("newer", t).await.unwrap();

        let viewer = get(&fx, Role::Viewer, "/api/v1/unknown-agents").await;
        assert_eq!(viewer.status, StatusCode::FORBIDDEN);
        let reply = get(&fx, Role::Operator, "/api/v1/unknown-agents").await;
        assert_eq!(reply.status, StatusCode::OK);
        let items = reply.json()["items"].as_array().unwrap().clone();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["host_name"], "newer");
        assert_eq!(items[0]["attempts"], 2);
        assert!(items[0]["first_seen"].as_str().unwrap().ends_with('Z'));
        assert!(items[0]["last_seen"].as_str().unwrap().ends_with('Z'));
        assert_eq!(items[1]["host_name"], "older");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_agents_are_capped_at_500(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        sqlx::query(
            "INSERT INTO unknown_agents (host_name, first_seen, last_seen, attempts) \
             SELECT 'agent-' || n, now(), now() - n * interval '1 second', 1 \
             FROM generate_series(1, 520) n",
        )
        .execute(&pool)
        .await
        .unwrap();
        let v = get(&fx, Role::Operator, "/api/v1/unknown-agents")
            .await
            .json();
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 500);
        assert_eq!(items[0]["host_name"], "agent-1");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_key_returns_it_once(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let reply = create_key(&fx, json!({"description": "rollout", "expires_in": "30d"})).await;
        assert_eq!(reply.status, StatusCode::CREATED);
        let body = reply.json();
        let key = body["key"].as_str().unwrap().to_owned();
        assert!(key.starts_with("cbx_ek_"));
        assert_eq!(body["description"], "rollout");
        assert_eq!(body["state"], "active");
        assert_eq!(body["revoked_at"], Value::Null);
        let id = body["id"].as_str().unwrap().to_owned();
        let expires = OffsetDateTime::parse(
            body["expires_at"].as_str().unwrap(),
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let days = (expires - OffsetDateTime::now_utc()).whole_days();
        assert!((29..=30).contains(&days), "{days}");

        // The list never shows the key, nor a value that resembles it or its hash.
        let list = get(&fx, Role::Admin, "/api/v1/enrollment-keys").await;
        let text = String::from_utf8(list.body.clone()).unwrap();
        assert!(!text.contains(&key) && !text.contains("cbx_ek_") && !text.contains("hash"));
        let items = list.json()["items"].as_array().unwrap().clone();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], id.as_str());
        assert!(items[0].get("key").is_none());
        assert_eq!(items[0]["state"], "active");

        // The key enrols an agent through the agent API.
        let enrol = json!({
            "host": fx.host, "enrollment_key": key, "agent_version": "0.1.0"
        })
        .to_string();
        let reply = fx
            .call("POST", "/api/agent/v1/enroll", None, &[], Some(&enrol))
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.json());
        assert!(reply.json()["credential"].as_str().is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn key_input_is_validated(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let long = "x".repeat(201);
        let max = "x".repeat(200);
        for (body, ok) in [
            (json!({"description": "d", "expires_in": "1h"}), true),
            (json!({"description": "d", "expires_in": "365d"}), true),
            (json!({"description": max, "expires_in": "1d"}), true),
            (json!({"description": "d", "expires_in": "59m"}), false),
            (json!({"description": "d", "expires_in": "366d"}), false),
            (json!({"description": "d", "expires_in": "8760h"}), true),
            (json!({"description": "d", "expires_in": "8761h"}), false),
            (json!({"description": "d", "expires_in": "0d"}), false),
            (json!({"description": "d", "expires_in": "soon"}), false),
            (json!({"description": "d", "expires_in": 30}), false),
            (json!({"description": "d"}), false),
            (json!({"expires_in": "1d"}), false),
            (json!({"description": "", "expires_in": "1d"}), false),
            (json!({"description": long, "expires_in": "1d"}), false),
            (json!({"description": "a\nb", "expires_in": "1d"}), false),
            (json!({"description": "a\u{0}b", "expires_in": "1d"}), false),
            (
                json!({"description": "d", "expires_in": "1d", "extra": 1}),
                false,
            ),
        ] {
            let reply = create_key(&fx, body.clone()).await;
            if ok {
                assert_eq!(reply.status, StatusCode::CREATED, "{body}");
            } else {
                assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body}");
                assert_eq!(reply.error_code(), "invalid", "{body}");
            }
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn key_expiry_bounds(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        for expires_in in ["59m", "366d"] {
            let reply =
                create_key(&fx, json!({"description": "d", "expires_in": expires_in})).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{expires_in}");
        }
        let list = get(&fx, Role::Admin, "/api/v1/enrollment-keys")
            .await
            .json();
        assert_eq!(list["items"], json!([]));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn revoked_and_expired_states(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let mut ids = Vec::new();
        for description in ["active", "revoked", "expired"] {
            let body = create_key(&fx, json!({"description": description, "expires_in": "1d"}))
                .await
                .json();
            ids.push(body["id"].as_str().unwrap().to_owned());
        }
        let token = fx.session(Role::Admin).await;
        let uri = format!("/api/v1/enrollment-keys/{}", ids[1]);
        let reply = fx.call("DELETE", &uri, Some(&token), &[], None).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        assert!(reply.body.is_empty());
        // Revoking again is fine.
        let reply = fx.call("DELETE", &uri, Some(&token), &[], None).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        // An expired key that is also revoked is revoked.
        sqlx::query("UPDATE enrollment_keys SET expires_at = now() - interval '1 hour' WHERE description IN ('expired', 'revoked')")
            .execute(&pool)
            .await
            .unwrap();

        let list = get(&fx, Role::Admin, "/api/v1/enrollment-keys")
            .await
            .json();
        let state_of = |description: &str| {
            list["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|k| k["description"] == description)
                .map(|k| k["state"].clone())
        };
        assert_eq!(state_of("active"), Some(json!("active")));
        assert_eq!(state_of("revoked"), Some(json!("revoked")));
        assert_eq!(state_of("expired"), Some(json!("expired")));
        let revoked = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["description"] == "revoked")
            .unwrap();
        assert!(revoked["revoked_at"].as_str().unwrap().ends_with('Z'));
        // Newest first.
        assert_eq!(list["items"][0]["description"], "expired");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn revoke_unknown_is_not_found(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let token = fx.session(Role::Admin).await;
        for id in [Uuid::new_v4().to_string(), "not-a-uuid".to_owned()] {
            let uri = format!("/api/v1/enrollment-keys/{id}");
            let reply = fx.call("DELETE", &uri, Some(&token), &[], None).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{id}");
            assert_eq!(reply.error_code(), "not_found");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_revoked_key_no_longer_enrols(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let body = create_key(&fx, json!({"description": "d", "expires_in": "1d"}))
            .await
            .json();
        let key = body["key"].as_str().unwrap();
        let token = fx.session(Role::Admin).await;
        let uri = format!("/api/v1/enrollment-keys/{}", body["id"].as_str().unwrap());
        fx.call("DELETE", &uri, Some(&token), &[], None).await;
        let enrol = json!({"host": fx.host, "enrollment_key": key, "agent_version": "0.1.0"});
        let reply = fx
            .call(
                "POST",
                "/api/agent/v1/enroll",
                None,
                &[],
                Some(&enrol.to_string()),
            )
            .await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn release_lets_the_host_enrol_again(pool: PgPool) {
        let mut config = (*config(true, None)).clone();
        // Releasing changes no configuration: it is allowed with GitOps on.
        config.gitops.enabled = true;
        let fx = Fixture::with_config(pool, Arc::new(config)).await;
        let (_, key) = fx
            .store
            .create_enrollment_key("t", OffsetDateTime::now_utc() + Duration::hours(1))
            .await
            .unwrap();
        let enrol =
            json!({"host": fx.host, "enrollment_key": key, "agent_version": "0.1.0"}).to_string();
        let first = fx
            .call("POST", "/api/agent/v1/enroll", None, &[], Some(&enrol))
            .await;
        assert_eq!(first.status, StatusCode::OK);
        let again = fx
            .call("POST", "/api/agent/v1/enroll", None, &[], Some(&enrol))
            .await;
        assert_eq!(again.status, StatusCode::CONFLICT);

        let token = fx.session(Role::Admin).await;
        let uri = format!("/api/v1/hosts/{}/release", fx.host);
        let reply = fx.call("POST", &uri, Some(&token), &[], Some("{}")).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        let again = fx
            .call("POST", "/api/agent/v1/enroll", None, &[], Some(&enrol))
            .await;
        assert_eq!(again.status, StatusCode::OK);
        // The old credential no longer works.
        let old = first.json()["credential"].as_str().unwrap().to_owned();
        assert!(fx.store.host_for_credential(&old).await.unwrap().is_none());
        // A host without a binding is released too.
        fx.call("POST", &uri, Some(&token), &[], Some("{}")).await;
        let reply = fx.call("POST", &uri, Some(&token), &[], Some("{}")).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn release_of_an_unknown_or_inactive_host_is_not_found(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let gone = fx.store.create_host("gone").await.unwrap();
        fx.store.set_host_active(gone.id, false).await.unwrap();
        let token = fx.session(Role::Admin).await;
        for name in ["nobody", "gone", "Bad Name", "-x"] {
            let uri = format!("/api/v1/hosts/{name}/release").replace(' ', "%20");
            let reply = fx.call("POST", &uri, Some(&token), &[], Some("{}")).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{name}");
            assert_eq!(reply.error_code(), "not_found", "{name}");
        }
    }
}
