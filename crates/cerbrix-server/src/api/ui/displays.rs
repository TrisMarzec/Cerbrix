//! Wall displays: the list for the desk, the display tokens, and the two routes a wall
//! monitor polls with its token.
//!
//! A monitor polls `GET /api/v1/display/{id}` every few seconds. It costs a fixed number of
//! queries (token lookup, the display, the dashboards with their hosts and open problems,
//! the sync state), however many dashboards and hosts exist.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cerbrix_common::{DisplayFile, Show, SwitchOnAlarm, validate_name};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{ApiError, Entry, dashboards, forbidden};
use crate::api::AppState;
use crate::auth::{Admin, DisplayAccess, Viewer};
use crate::state::StateError;
use crate::view::display::{DisplayTarget, alarm_target, allowed_dashboards};

pub(super) fn entries() -> Vec<Entry> {
    vec![
        Entry::new("GET", "/api/v1/displays", list),
        Entry::new("POST", "/api/v1/displays/{id}/token", create_token),
        Entry::new("DELETE", "/api/v1/displays/{id}/token", revoke_token),
        Entry::new("GET", "/api/v1/display/{id}", state),
        Entry::new(
            "GET",
            "/api/v1/display/{id}/dashboards/{dashboard}",
            dashboard,
        ),
    ]
}

fn store_failed(what: &str, e: &StateError) -> ApiError {
    tracing::error!(error = %e, "{what}");
    ApiError::internal()
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such display")
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[derive(Serialize)]
struct DisplayItem<'a> {
    id: &'a str,
    name: &'a str,
    file: &'a str,
    show: &'a Show,
    switch_on_alarm: &'a Option<SwitchOnAlarm>,
    dashboards: &'a Option<Vec<String>>,
    /// Only when the token was created, never the token.
    token_created_at: Option<String>,
    #[serde(flatten)]
    alarm: DisplayTarget,
}

#[derive(Serialize)]
struct Items<'a> {
    items: Vec<DisplayItem<'a>>,
}

/// All displays with what each shows now: every target comes from one read of the
/// dashboards, hosts and problems.
async fn list(State(app): State<AppState>, Viewer(_): Viewer) -> Result<Response, ApiError> {
    let (displays, inputs, created) = tokio::try_join!(
        app.store.displays(),
        app.store.board_inputs(),
        app.store.display_tokens_created(),
    )
    .map_err(|e| store_failed("listing displays failed", &e))?;
    let items = displays
        .iter()
        .map(|(id, file, display)| DisplayItem {
            id,
            name: &display.name,
            file,
            show: &display.show,
            switch_on_alarm: &display.switch_on_alarm,
            dashboards: &display.dashboards,
            token_created_at: created.get(id).copied().map(rfc3339),
            alarm: alarm_target(display, &inputs),
        })
        .collect();
    // The items borrow from the data read above, so the answer is made here.
    Ok(Json(Items { items }).into_response())
}

#[derive(Serialize)]
struct TokenOut {
    /// Shown once; only its hash is stored.
    token: String,
}

/// Creates the display's token, replacing an existing one. Allowed with GitOps on: a token
/// is not configuration.
async fn create_token(
    State(app): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<TokenOut>), ApiError> {
    if validate_name(&id).is_err() {
        return Err(not_found());
    }
    let token = app
        .store
        .create_display_token(&id, OffsetDateTime::now_utc())
        .await
        .map_err(|e| match e {
            StateError::NotFound => not_found(),
            e => store_failed("creating a display token failed", &e),
        })?;
    tracing::info!(admin = %admin.name, display = %id, "display token created");
    Ok((StatusCode::CREATED, Json(TokenOut { token })))
}

/// Revokes the display's token. A display without a token, or one that does not exist, is a
/// 204 too; only an id that cannot be a display id is a 404.
async fn revoke_token(
    State(app): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if validate_name(&id).is_err() {
        return Err(not_found());
    }
    app.store
        .revoke_display_token(&id)
        .await
        .map_err(|e| store_failed("revoking a display token failed", &e))?;
    tracing::info!(admin = %admin.name, display = %id, "display token revoked");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct SyncOut {
    status: &'static str,
    active_commit: Option<String>,
    /// How many issues the last sync found; their text stays on the desk.
    issues: usize,
}

#[derive(Serialize)]
struct StateOut<'a> {
    id: &'a str,
    name: &'a str,
    show: &'a Show,
    switch_on_alarm: &'a Option<SwitchOnAlarm>,
    allowed: Vec<String>,
    #[serde(flatten)]
    alarm: DisplayTarget,
    sync: SyncOut,
}

/// The display `id` of the active configuration; 404 for an unknown one.
async fn find(app: &AppState, id: &str) -> Result<DisplayFile, ApiError> {
    if validate_name(id).is_err() {
        return Err(not_found());
    }
    app.store
        .display(id)
        .await
        .map_err(|e| store_failed("reading a display failed", &e))?
        .map(|(_, display)| display)
        .ok_or_else(not_found)
}

/// What a monitor needs on every poll: its setting, where an alarm sends it, which
/// dashboards it may open and the sync state.
async fn state(State(app): State<AppState>, access: DisplayAccess) -> Result<Response, ApiError> {
    let id = access.display_id;
    if validate_name(&id).is_err() {
        return Err(not_found());
    }
    let (display, inputs, sync) = tokio::try_join!(
        app.store.display(&id),
        app.store.board_inputs(),
        app.store.sync_state(),
    )
    .map_err(|e| store_failed("reading a display failed", &e))?;
    let (_, display) = display.ok_or_else(not_found)?;
    let all: Vec<String> = inputs
        .dashboards
        .iter()
        .map(|(id, _, _)| id.clone())
        .collect();
    let out = StateOut {
        id: &id,
        name: &display.name,
        show: &display.show,
        switch_on_alarm: &display.switch_on_alarm,
        allowed: allowed_dashboards(&display, &all),
        alarm: alarm_target(&display, &inputs),
        sync: SyncOut {
            status: sync.status.as_str(),
            active_commit: sync.active_commit,
            issues: sync.issues.len(),
        },
    };
    Ok(Json(out).into_response())
}

/// The dashboard answer of the desk, for a dashboard the display may show.
async fn dashboard(
    State(app): State<AppState>,
    access: DisplayAccess,
    Path((_, dashboard)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let display = find(&app, &access.display_id).await?;
    let all = app
        .store
        .dashboard_ids()
        .await
        .map_err(|e| store_failed("listing dashboards failed", &e))?;
    if !allowed_dashboards(&display, &all).contains(&dashboard) {
        return Err(forbidden("this display may not show that dashboard"));
    }
    dashboards::answer(&app, &dashboard).await
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use crate::api::ui::testing::*;
    use crate::auth::Role;

    async fn get(fx: &Fixture, uri: &str) -> Reply {
        let token = fx.session(Role::Viewer).await;
        fx.call("GET", uri, Some(&token), &[], None).await
    }

    async fn put_doc(fx: &Fixture, kind: &str, id: &str, body: &str) {
        sqlx::query(
            "INSERT INTO config_documents (kind, id, source_file, body) \
             VALUES ($1, $2, $3, $4::jsonb)",
        )
        .bind(kind)
        .bind(id)
        .bind(format!("{kind}s/{id}.json"))
        .bind(body)
        .execute(&fx.pool)
        .await
        .unwrap();
    }

    /// A dashboard that selects the hosts of one group.
    async fn board(fx: &Fixture, id: &str, priority: i32, group: &str) {
        let body = json!({"name": id, "priority": priority, "hosts": {"groups": [group]}});
        put_doc(fx, "dashboard", id, &body.to_string()).await;
    }

    async fn host_in(fx: &Fixture, name: &str, group: &str) {
        fx.store.create_host(name).await.unwrap();
        sqlx::query("UPDATE hosts SET groups = $2 WHERE name = $1")
            .bind(name)
            .bind(vec![group.to_owned()])
            .execute(&fx.pool)
            .await
            .unwrap();
    }

    /// An open problem on `host`; returns its id.
    async fn alarm(fx: &Fixture, host: &str, severity: &str, acknowledged: bool) -> Uuid {
        let host = fx.store.host_by_name(host).await.unwrap().unwrap().id;
        let id = Uuid::new_v4();
        let started = OffsetDateTime::now_utc() - Duration::hours(1);
        sqlx::query(
            "INSERT INTO problems (id, host_id, trigger_name, instance, severity, summary, \
             started_at, acknowledged_at) VALUES ($1, $2, 'T', '', $3, 'summary', $4, $5)",
        )
        .bind(id)
        .bind(host)
        .bind(severity)
        .bind(started)
        .bind(acknowledged.then_some(started))
        .execute(&fx.pool)
        .await
        .unwrap();
        id
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
    async fn displays_list_shape_with_target_and_queue(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        board(&fx, "a", 5, "ga").await;
        board(&fx, "b", 9, "gb").await;
        board(&fx, "quiet", 99, "gq").await;
        host_in(&fx, "host-a", "ga").await;
        host_in(&fx, "host-b", "gb").await;
        let cause_a = alarm(&fx, "host-a", "critical", false).await;
        let cause_b = alarm(&fx, "host-b", "critical", false).await;
        put_doc(
            &fx,
            "display",
            "wall",
            r#"{"name": "Wall", "show": {"rotate": ["a", "b"], "every": "30s"},
                "switch_on_alarm": {"min_severity": "high"}}"#,
        )
        .await;
        put_doc(
            &fx,
            "display",
            "plain",
            r#"{"name": "Plain", "show": {"dashboard": "a"}, "dashboards": ["a"]}"#,
        )
        .await;

        let reply = get(&fx, "/api/v1/displays").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(keys(&body), ["items"]);
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        // By id.
        assert_eq!(items[0]["id"], "plain");
        assert_eq!(items[1]["id"], "wall");
        let wall = &items[1];
        assert_eq!(
            keys(wall),
            [
                "causes",
                "dashboards",
                "file",
                "id",
                "name",
                "show",
                "switch_on_alarm",
                "target",
                "token_created_at",
                "waiting"
            ]
        );
        assert_eq!(wall["file"], "displays/wall.json");
        assert_eq!(wall["name"], "Wall");
        assert_eq!(wall["show"], json!({"rotate": ["a", "b"], "every": "30s"}));
        assert_eq!(wall["switch_on_alarm"], json!({"min_severity": "high"}));
        assert_eq!(wall["dashboards"], Value::Null);
        assert_eq!(wall["token_created_at"], Value::Null);
        assert_eq!(wall["target"], "b");
        assert_eq!(wall["causes"], json!([cause_b]));
        assert_eq!(wall["waiting"], json!(["a"]));
        assert_ne!(cause_a, cause_b);
        // A display that does not switch has no alarm target.
        let plain = &items[0];
        assert_eq!(plain["target"], Value::Null);
        assert_eq!(plain["causes"], json!([]));
        assert_eq!(plain["waiting"], json!([]));
        assert_eq!(plain["dashboards"], json!(["a"]));
        assert_eq!(plain["switch_on_alarm"], Value::Null);

        // The token's creation time shows, the token never.
        let token = fx
            .store
            .create_display_token("wall", OffsetDateTime::now_utc())
            .await
            .unwrap();
        let reply = get(&fx, "/api/v1/displays").await;
        let text = String::from_utf8(reply.body.clone()).unwrap();
        assert!(!text.contains(&token));
        let at = reply.json()["items"][1]["token_created_at"].clone();
        assert!(at.as_str().is_some_and(|t| t.ends_with('Z')), "{at}");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_token_returns_it_once(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.ensure_dashboard().await;
        fx.ensure_display().await;
        let uri = format!("/api/v1/displays/{}/token", fx.display);
        let admin = fx.session(Role::Admin).await;

        let viewer = fx.session(Role::Viewer).await;
        let reply = fx.call("POST", &uri, Some(&viewer), &[], Some("{}")).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);

        let reply = fx.call("POST", &uri, Some(&admin), &[], Some("{}")).await;
        assert_eq!(reply.status, StatusCode::CREATED);
        let body = reply.json();
        assert_eq!(keys(&body), ["token"]);
        let first = body["token"].as_str().unwrap().to_owned();
        assert!(first.starts_with("cbx_dt_"));
        // It works, and it is not in the list.
        let state = format!("/api/v1/display/{}", fx.display);
        assert_eq!(
            fx.call_with_token(&state, &first).await.status,
            StatusCode::OK
        );
        let list = get(&fx, "/api/v1/displays").await;
        assert!(!String::from_utf8(list.body).unwrap().contains(&first));

        // A new token replaces the old one.
        let reply = fx.call("POST", &uri, Some(&admin), &[], Some("{}")).await;
        let second = reply.json()["token"].as_str().unwrap().to_owned();
        assert_ne!(first, second);
        let old = fx.call_with_token(&state, &first).await;
        assert_eq!(old.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            fx.call_with_token(&state, &second).await.status,
            StatusCode::OK
        );

        // Unknown display, and an id that cannot be one.
        for id in ["nobody", "Not-A-Name"] {
            let uri = format!("/api/v1/displays/{id}/token");
            let reply = fx.call("POST", &uri, Some(&admin), &[], Some("{}")).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{id}");
            assert_eq!(reply.error_code(), "not_found", "{id}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn revoked_token_is_unauthorized(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.ensure_dashboard().await;
        fx.ensure_display().await;
        let token = fx.display_token().await;
        let state = format!("/api/v1/display/{}", fx.display);
        assert_eq!(
            fx.call_with_token(&state, &token).await.status,
            StatusCode::OK
        );

        let admin = fx.session(Role::Admin).await;
        let uri = format!("/api/v1/displays/{}/token", fx.display);
        let operator = fx.session(Role::Operator).await;
        let reply = fx.call("DELETE", &uri, Some(&operator), &[], None).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        let reply = fx.call("DELETE", &uri, Some(&admin), &[], None).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        let reply = fx.call_with_token(&state, &token).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert_eq!(reply.error_code(), "unauthorized");

        // Again, and for a display that does not exist: still 204.
        let reply = fx.call("DELETE", &uri, Some(&admin), &[], None).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        let nobody = "/api/v1/displays/nobody/token";
        let reply = fx.call("DELETE", nobody, Some(&admin), &[], None).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        let bad = "/api/v1/displays/Bad_Id/token";
        let reply = fx.call("DELETE", bad, Some(&admin), &[], None).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn display_state_with_token(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.ensure_dashboard().await;
        fx.ensure_display().await;
        board(&fx, "other", 1, "go").await;
        let token = fx.display_token().await;
        let uri = format!("/api/v1/display/{}", fx.display);

        let reply = fx.call_with_token(&uri, &token).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.header("cache-control"), Some("no-store"));
        let v = reply.json();
        assert_eq!(
            keys(&v),
            [
                "allowed",
                "causes",
                "id",
                "name",
                "show",
                "switch_on_alarm",
                "sync",
                "target",
                "waiting"
            ]
        );
        assert_eq!(v["id"], fx.display);
        assert_eq!(v["name"], "Fixture wall");
        assert_eq!(v["show"], json!({"dashboard": "fixture-board"}));
        assert_eq!(v["switch_on_alarm"], json!({"min_severity": "critical"}));
        assert_eq!(v["allowed"], json!(["fixture-board", "other"]));
        assert_eq!(v["target"], Value::Null);
        assert_eq!(
            v["sync"],
            json!({"status": "never", "active_commit": null, "issues": 0})
        );

        // The fixture dashboard selects every host, so the alarm is on both dashboards and the
        // higher priority wins; the sync issues are
        // counted, their text stays off the wall.
        host_in(&fx, "host-o", "go").await;
        let cause = alarm(&fx, "host-o", "critical", false).await;
        let issues = [crate::gitops::SyncIssue {
            file: Some("hosts/web-01.json".into()),
            line: None,
            column: None,
            reason: "secret detail".into(),
        }];
        fx.store
            .record_sync_failure(Some("abc"), &issues, OffsetDateTime::now_utc())
            .await
            .unwrap();
        let reply = fx.call_with_token(&uri, &token).await;
        let text = String::from_utf8(reply.body.clone()).unwrap();
        assert!(!text.contains("secret detail") && !text.contains("web-01"));
        let v = reply.json();
        assert_eq!(v["target"], "other");
        assert_eq!(v["causes"], json!([cause]));
        assert_eq!(v["waiting"], json!(["fixture-board"]));
        assert_eq!(v["sync"]["status"], "out_of_sync");
        assert_eq!(v["sync"]["issues"], 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn users_see_every_display_and_unknown_is_404_or_401(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.ensure_dashboard().await;
        fx.ensure_display().await;
        let uri = format!("/api/v1/display/{}", fx.display);
        for role in [Role::Viewer, Role::Operator, Role::Admin] {
            let session = fx.session(role).await;
            let reply = fx.call("GET", &uri, Some(&session), &[], None).await;
            assert_eq!(reply.status, StatusCode::OK, "{role}");
        }
        let session = fx.session(Role::Viewer).await;
        for id in ["nobody", "Not-A-Name"] {
            let uri = format!("/api/v1/display/{id}");
            let reply = fx.call("GET", &uri, Some(&session), &[], None).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{id}");
            let token = fx.display_token().await;
            let reply = fx.call_with_token(&uri, &token).await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{id}");
        }
        // A session wins over a token that is wrong.
        let wrong = [("authorization", "Bearer cbx_dt_nonsense")];
        let reply = fx.call("GET", &uri, Some(&session), &wrong, None).await;
        assert_eq!(reply.status, StatusCode::OK);
        // An invalid session does not hide a valid token.
        let token = fx.display_token().await;
        let bearer = format!("Bearer {token}");
        let headers = [("authorization", bearer.as_str())];
        let reply = fx
            .call("GET", &uri, Some("cbx_se_unknown"), &headers, None)
            .await;
        assert_eq!(reply.status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn token_of_another_display_is_unauthorized(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.ensure_dashboard().await;
        fx.ensure_display().await;
        put_doc(
            &fx,
            "display",
            "second",
            r#"{"name": "Second", "show": {"dashboard": "fixture-board"}}"#,
        )
        .await;
        let mine = fx.display_token().await;
        let theirs = fx
            .store
            .create_display_token("second", OffsetDateTime::now_utc())
            .await
            .unwrap();
        let uri = format!("/api/v1/display/{}", fx.display);
        let wrong_display = fx.call_with_token(&uri, &theirs).await;
        let unknown = fx.call_with_token(&uri, "cbx_dt_unknown").await;
        let missing = fx.call("GET", &uri, None, &[], None).await;
        let no_scheme = [("authorization", mine.as_str())];
        let malformed = fx.call("GET", &uri, None, &no_scheme, None).await;
        for reply in [&wrong_display, &unknown, &missing, &malformed] {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            // The same answer whatever is wrong with the token.
            assert_eq!(reply.body, wrong_display.body);
        }
        // And the route of the dashboards.
        let dash = format!("/api/v1/display/{}/dashboards/fixture-board", fx.display);
        let reply = fx.call_with_token(&dash, &theirs).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            fx.call_with_token(&dash, &mine).await.status,
            StatusCode::OK
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn token_cannot_read_other_routes(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.ensure_dashboard().await;
        fx.ensure_display().await;
        fx.ensure_problem().await;
        let token = fx.display_token().await;
        for uri in [
            "/api/v1/hosts".to_owned(),
            format!("/api/v1/hosts/{}", fx.host),
            "/api/v1/problems".to_owned(),
            format!("/api/v1/problems/{}", fx.problem),
            "/api/v1/dashboards".to_owned(),
            format!("/api/v1/dashboards/{}", fx.dashboard),
            "/api/v1/users".to_owned(),
            "/api/v1/status".to_owned(),
            "/api/v1/session".to_owned(),
            "/api/v1/displays".to_owned(),
        ] {
            let reply = fx.call_with_token(&uri, &token).await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{uri}");
            assert_eq!(reply.error_code(), "unauthorized", "{uri}");
        }
        let bearer = format!("Bearer {token}");
        let headers = [("authorization", bearer.as_str())];
        for (method, uri) in [
            ("POST", format!("/api/v1/displays/{}/token", fx.display)),
            ("DELETE", format!("/api/v1/displays/{}/token", fx.display)),
            ("POST", format!("/api/v1/problems/{}/ack", fx.problem)),
        ] {
            let reply = fx.call(method, &uri, None, &headers, Some("{}")).await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        }
        // The token is still there: nothing above could revoke it.
        let state = format!("/api/v1/display/{}", fx.display);
        assert_eq!(
            fx.call_with_token(&state, &token).await.status,
            StatusCode::OK
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn dashboard_outside_allowed_is_forbidden(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.ensure_dashboard().await;
        board(&fx, "customer-b", 1, "gb").await;
        put_doc(
            &fx,
            "display",
            "limited",
            r#"{"name": "Limited", "show": {"dashboard": "fixture-board"},
                "switch_on_alarm": {}, "dashboards": ["fixture-board"]}"#,
        )
        .await;
        let token = fx
            .store
            .create_display_token("limited", OffsetDateTime::now_utc())
            .await
            .unwrap();
        let session = fx.session(Role::Viewer).await;
        for uri in [
            "/api/v1/display/limited/dashboards/customer-b",
            "/api/v1/display/limited/dashboards/nonexistent",
            "/api/v1/display/limited/dashboards/Not%20A%20Name",
        ] {
            let reply = fx.call_with_token(uri, &token).await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN, "{uri}");
            assert_eq!(reply.error_code(), "forbidden", "{uri}");
            let reply = fx.call("GET", uri, Some(&session), &[], None).await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN, "{uri} as a user");
        }
        // What it may show is served; for a user too.
        let uri = "/api/v1/display/limited/dashboards/fixture-board";
        assert_eq!(fx.call_with_token(uri, &token).await.status, StatusCode::OK);
        let reply = fx.call("GET", uri, Some(&session), &[], None).await;
        assert_eq!(reply.status, StatusCode::OK);
        // An unknown display is 404 for a user.
        let uri = "/api/v1/display/nobody/dashboards/fixture-board";
        let reply = fx.call("GET", uri, Some(&session), &[], None).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn display_dashboard_data_matches_the_dashboards_route(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        host_in(&fx, "extra", "production").await;
        alarm(&fx, "extra", "high", false).await;
        let token = fx
            .store
            .create_display_token("noc", OffsetDateTime::now_utc())
            .await
            .unwrap();
        let desk = get(&fx, "/api/v1/dashboards/production").await;
        assert_eq!(desk.status, StatusCode::OK);
        let wall = fx
            .call_with_token("/api/v1/display/noc/dashboards/production", &token)
            .await;
        assert_eq!(wall.status, StatusCode::OK);
        assert_eq!(wall.json(), desk.json());
        assert!(!desk.json()["hosts"].as_array().unwrap().is_empty());
    }
}
