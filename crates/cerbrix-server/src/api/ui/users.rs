//! The users of the web UI, managed by admins.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{ApiError, ApiJson, Entry, Listed, Page, hash_failed, invalid, store_failed};
use crate::api::AppState;
use crate::auth::{Admin, Role, generate_password, validate_password, validate_user_name};
use crate::state::{StateError, User};
use crate::view::rfc3339;

pub(super) fn entries() -> Vec<Entry> {
    vec![
        Entry::new("GET", "/api/v1/users", list),
        Entry::new("POST", "/api/v1/users", create),
        Entry::new("PATCH", "/api/v1/users/{id}", update),
        Entry::new("POST", "/api/v1/users/{id}/password", reset_password),
        Entry::new("DELETE", "/api/v1/users/{id}", remove),
    ]
}

/// A user as the API shows it. There is no password hash in it; `password` is set only in
/// the answer to creating a user without one.
#[derive(Serialize)]
struct UserOut {
    id: String,
    name: String,
    role: &'static str,
    disabled: bool,
    created_at: String,
    password_changed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

impl UserOut {
    fn new(user: &User, password: Option<String>) -> Self {
        Self {
            id: user.id.to_string(),
            name: user.name.clone(),
            role: user.role.as_str(),
            disabled: user.disabled,
            created_at: rfc3339(user.created_at),
            password_changed_at: rfc3339(user.password_changed_at),
            password,
        }
    }
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such user")
}

fn conflict(message: &'static str) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "conflict", message)
}

/// A malformed id cannot name a user.
fn parse_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| not_found())
}

/// The answer to a store error of a change of an existing user.
fn change_failed(what: &str, e: StateError) -> ApiError {
    match e {
        StateError::NotFound => not_found(),
        StateError::Conflict(_) => conflict("the last enabled admin cannot be changed"),
        e => store_failed(what, &e),
    }
}

fn parse_role(raw: &str) -> Result<Role, ApiError> {
    raw.parse()
        .map_err(|_| invalid("the role must be viewer, operator or admin"))
}

/// The password to store and, when the caller gave none, the generated one to return once.
fn password_or_generated(given: Option<String>) -> Result<(String, Option<String>), ApiError> {
    match given {
        Some(password) => {
            validate_password(&password).map_err(|_| {
                invalid("the password must have 12 to 256 characters and no NUL character")
            })?;
            Ok((password, None))
        }
        None => {
            let password = generate_password();
            Ok((password.clone(), Some(password)))
        }
    }
}

async fn hash(app: &AppState, password: String) -> Result<String, ApiError> {
    app.passwords.hash(password).await.map_err(hash_failed)
}

async fn list(
    State(app): State<AppState>,
    Admin(_): Admin,
    page: Page,
) -> Result<Json<Listed<UserOut>>, ApiError> {
    let (users, total) = app
        .store
        .users_page(page.limit, page.offset)
        .await
        .map_err(|e| store_failed("listing users failed", &e))?;
    Ok(Json(Listed {
        items: users.iter().map(|u| UserOut::new(u, None)).collect(),
        total,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    name: String,
    role: String,
    password: Option<String>,
}

async fn create(
    State(app): State<AppState>,
    Admin(admin): Admin,
    ApiJson(body): ApiJson<CreateBody>,
) -> Result<Response, ApiError> {
    validate_user_name(&body.name).map_err(|_| {
        invalid(
            "invalid name: use 1 to 63 characters from a-z, 0-9, '.', '_' and '-', \
             starting with a letter or digit",
        )
    })?;
    let role = parse_role(&body.role)?;
    let (password, generated) = password_or_generated(body.password)?;
    let hash = hash(&app, password).await?;
    let user = app
        .store
        .create_user(&body.name, role, &hash, OffsetDateTime::now_utc())
        .await
        .map_err(|e| match e {
            StateError::Conflict(_) => conflict("a user with this name already exists"),
            e => store_failed("creating a user failed", &e),
        })?;
    tracing::info!(
        admin = %admin.name,
        user = %user.name,
        role = %user.role,
        generated_password = generated.is_some(),
        "user created"
    );
    Ok((StatusCode::CREATED, Json(UserOut::new(&user, generated))).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    role: Option<String>,
    disabled: Option<bool>,
}

/// Changes the role and/or disables or enables a user. Both are applied together or, when
/// the last enabled admin would be lost, not at all.
async fn update(
    State(app): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> Result<Json<UserOut>, ApiError> {
    let id = parse_id(&id)?;
    let role = body.role.as_deref().map(parse_role).transpose()?;
    if role.is_none() && body.disabled.is_none() {
        return Err(invalid("give a role, disabled or both"));
    }
    let user = app
        .store
        .update_user(id, role, body.disabled)
        .await
        .map_err(|e| change_failed("changing a user failed", e))?;
    tracing::info!(
        admin = %admin.name,
        user = %user.name,
        role = ?role.map(Role::as_str),
        disabled = ?body.disabled,
        "user changed"
    );
    Ok(Json(UserOut::new(&user, None)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetBody {
    password: Option<String>,
}

/// Sets a new password, the given one or a generated one (returned once), and ends all
/// sessions of the user, the caller's own included when the caller resets their own.
async fn reset_password(
    State(app): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ResetBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_id(&id)?;
    let (password, generated) = password_or_generated(body.password)?;
    let user = app
        .store
        .user_by_id(id)
        .await
        .map_err(|e| store_failed("loading a user failed", &e))?
        .ok_or_else(not_found)?;
    let hash = hash(&app, password).await?;
    app.store
        .set_user_password(id, &hash, OffsetDateTime::now_utc())
        .await
        .map_err(|e| change_failed("resetting a password failed", e))?;
    tracing::info!(
        admin = %admin.name,
        user = %user.name,
        generated_password = generated.is_some(),
        "password reset"
    );
    Ok(Json(match generated {
        Some(password) => serde_json::json!({ "password": password }),
        None => serde_json::json!({}),
    }))
}

async fn remove(
    State(app): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_id(&id)?;
    let user = app
        .store
        .user_by_id(id)
        .await
        .map_err(|e| store_failed("loading a user failed", &e))?
        .ok_or_else(not_found)?;
    app.store
        .delete_user(id)
        .await
        .map_err(|e| change_failed("deleting a user failed", e))?;
    tracing::info!(admin = %admin.name, user = %user.name, "user deleted");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use uuid::Uuid;

    use crate::api::ui::testing::*;
    use crate::auth::Role;

    fn url(id: Uuid) -> String {
        format!("/api/v1/users/{id}")
    }

    async fn login(fx: &Fixture, name: &str, password: &str) -> Reply {
        let body = json!({ "name": name, "password": password }).to_string();
        fx.call("POST", "/api/v1/session", None, &[], Some(&body))
            .await
    }

    /// The session token a successful login set.
    fn token_of(reply: &Reply) -> String {
        reply
            .header("set-cookie")
            .and_then(|c| c.strip_prefix("cerbrix_session="))
            .and_then(|c| c.split(';').next())
            .expect("a session cookie")
            .to_owned()
    }

    async fn create(fx: &Fixture, token: &str, body: Value) -> Reply {
        fx.call(
            "POST",
            "/api/v1/users",
            Some(token),
            &[],
            Some(&body.to_string()),
        )
        .await
    }

    async fn patch(fx: &Fixture, token: &str, id: Uuid, body: Value) -> Reply {
        fx.call("PATCH", &url(id), Some(token), &[], Some(&body.to_string()))
            .await
    }

    async fn reset(fx: &Fixture, token: &str, id: Uuid, body: &str) -> Reply {
        fx.call(
            "POST",
            &format!("{}/password", url(id)),
            Some(token),
            &[],
            Some(body),
        )
        .await
    }

    async fn sessions_of(fx: &Fixture, id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id = $1")
            .bind(id)
            .fetch_one(&fx.pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_user_with_generated_password(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        let reply = create(&fx, &admin, json!({ "name": "newbie", "role": "operator" })).await;
        assert_eq!(reply.status, StatusCode::CREATED);
        let body = reply.json();
        assert_eq!(body["name"], "newbie");
        assert_eq!(body["role"], "operator");
        assert_eq!(body["disabled"], false);
        assert!(body["id"].as_str().is_some());
        assert!(body["created_at"].as_str().unwrap().ends_with('Z'));
        assert!(body["password_changed_at"].as_str().is_some());
        let password = body["password"].as_str().expect("a generated password");
        assert_eq!(login(&fx, "newbie", password).await.status, StatusCode::OK);
        // The password is shown once: the list does not have it.
        let list = fx
            .call("GET", "/api/v1/users", Some(&admin), &[], None)
            .await;
        assert_eq!(list.status, StatusCode::OK);
        assert!(!String::from_utf8_lossy(&list.body).contains(password));
        assert!(
            list.json()["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|u| u.get("password").is_none())
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_supplied_password_is_not_echoed(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        let secret = "a long enough secret";
        let reply = create(
            &fx,
            &admin,
            json!({ "name": "chosen", "role": "viewer", "password": secret }),
        )
        .await;
        assert_eq!(reply.status, StatusCode::CREATED);
        assert!(reply.json().get("password").is_none());
        assert!(!String::from_utf8_lossy(&reply.body).contains(secret));
        assert_eq!(login(&fx, "chosen", secret).await.status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_user_validation(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        for (body, field) in [
            (json!({ "name": "Bad Name", "role": "viewer" }), "name"),
            (json!({ "name": "", "role": "viewer" }), "name"),
            (json!({ "name": "ok", "role": "root" }), "role"),
            (
                json!({ "name": "ok", "role": "viewer", "password": "short" }),
                "password",
            ),
        ] {
            let reply = create(&fx, &admin, body.clone()).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(reply.error_code(), "invalid", "{body}");
            let message = reply.json()["message"].as_str().unwrap().to_owned();
            assert!(message.contains(field), "{message} should name {field}");
        }
        // Missing or unknown fields are invalid too.
        for body in [
            json!({ "name": "ok" }),
            json!({ "role": "viewer" }),
            json!({ "name": "ok", "role": "viewer", "extra": 1 }),
        ] {
            let reply = create(&fx, &admin, body.clone()).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body}");
        }
        // A taken name.
        let reply = create(&fx, &admin, json!({ "name": "viewer", "role": "admin" })).await;
        assert_eq!(reply.status, StatusCode::CONFLICT);
        assert_eq!(reply.error_code(), "conflict");
        assert_eq!(
            reply.json()["message"],
            "a user with this name already exists"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_is_sorted_by_name_and_paged(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        let reply = fx
            .call("GET", "/api/v1/users", Some(&admin), &[], None)
            .await;
        let body = reply.json();
        assert_eq!(body["total"], 4);
        let names: Vec<&str> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["admin", "operator", "subject", "viewer"]);
        let reply = fx
            .call(
                "GET",
                "/api/v1/users?limit=2&offset=1",
                Some(&admin),
                &[],
                None,
            )
            .await;
        let body = reply.json();
        assert_eq!(body["total"], 4);
        assert_eq!(body["items"].as_array().unwrap().len(), 2);
        assert_eq!(body["items"][0]["name"], "operator");
        let reply = fx
            .call("GET", "/api/v1/users?limit=0", Some(&admin), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn patch_role_and_disable(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        let reply = patch(&fx, &admin, fx.subject, json!({ "role": "operator" })).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["role"], "operator");
        assert_eq!(reply.json()["disabled"], false);
        let reply = patch(&fx, &admin, fx.subject, json!({ "disabled": true })).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["role"], "operator");
        assert_eq!(reply.json()["disabled"], true);
        // Both in one request.
        let reply = patch(
            &fx,
            &admin,
            fx.subject,
            json!({ "role": "viewer", "disabled": false }),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["role"], "viewer");
        assert_eq!(reply.json()["disabled"], false);
        let stored = fx.store.user_by_id(fx.subject).await.unwrap().unwrap();
        assert_eq!(stored.role, Role::Viewer);
        assert!(!stored.disabled);
        // Nothing to change, an unknown field, an unknown role, a wrong type.
        for body in [
            json!({}),
            json!({ "name": "x" }),
            json!({ "role": "root" }),
            json!({ "disabled": "yes" }),
            json!({ "role": null }),
        ] {
            let reply = patch(&fx, &admin, fx.subject, body.clone()).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(reply.error_code(), "invalid", "{body}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn disabled_user_is_logged_out(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        let viewer = fx.session(Role::Viewer).await;
        let ok = fx
            .call("GET", "/api/v1/session", Some(&viewer), &[], None)
            .await;
        assert_eq!(ok.status, StatusCode::OK);
        let reply = patch(&fx, &admin, fx.viewer, json!({ "disabled": true })).await;
        assert_eq!(reply.status, StatusCode::OK);
        let gone = fx
            .call("GET", "/api/v1/session", Some(&viewer), &[], None)
            .await;
        assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            login(&fx, "viewer", PASSWORD).await.status,
            StatusCode::UNAUTHORIZED
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn reset_password_ends_sessions(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        let viewer = fx.session(Role::Viewer).await;
        let reply = reset(&fx, &admin, fx.viewer, "{}").await;
        assert_eq!(reply.status, StatusCode::OK);
        let generated = reply.json()["password"].as_str().unwrap().to_owned();
        let gone = fx
            .call("GET", "/api/v1/session", Some(&viewer), &[], None)
            .await;
        assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            login(&fx, "viewer", PASSWORD).await.status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            login(&fx, "viewer", &generated).await.status,
            StatusCode::OK
        );

        // A supplied password: answered with `{}`.
        let chosen = "my own new password";
        let reply = reset(
            &fx,
            &admin,
            fx.viewer,
            &json!({ "password": chosen }).to_string(),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json(), json!({}));
        assert_eq!(login(&fx, "viewer", chosen).await.status, StatusCode::OK);

        // A short one is refused and changes nothing.
        let reply = reset(&fx, &admin, fx.viewer, r#"{"password":"short"}"#).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(login(&fx, "viewer", chosen).await.status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn an_admin_resets_their_own_password_and_loses_all_sessions(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        let reply = reset(&fx, &admin, fx.admin, "{}").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(sessions_of(&fx, fx.admin).await, 0);
        let gone = fx
            .call("GET", "/api/v1/session", Some(&admin), &[], None)
            .await;
        assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn last_admin_cannot_be_removed_through_the_api(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        let message = "the last enabled admin cannot be changed";
        for body in [
            json!({ "role": "viewer" }),
            json!({ "disabled": true }),
            json!({ "role": "operator", "disabled": true }),
        ] {
            let reply = patch(&fx, &admin, fx.admin, body.clone()).await;
            assert_eq!(reply.status, StatusCode::CONFLICT, "{body}");
            assert_eq!(reply.error_code(), "conflict");
            assert_eq!(reply.json()["message"], message);
        }
        let reply = fx
            .call("DELETE", &url(fx.admin), Some(&admin), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::CONFLICT);
        assert_eq!(reply.json()["message"], message);
        // Nothing changed, the session still works.
        let me = fx
            .call("GET", "/api/v1/session", Some(&admin), &[], None)
            .await;
        assert_eq!(me.status, StatusCode::OK);
        assert_eq!(me.json()["role"], "admin");
        // Keeping the role admin leaves an admin and is allowed.
        let reply = patch(&fx, &admin, fx.admin, json!({ "role": "admin" })).await;
        assert_eq!(reply.status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_refused_combination_changes_nothing(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        // `admin` is the only enabled admin; `operator` becomes a disabled admin.
        let reply = patch(
            &fx,
            &admin,
            fx.operator,
            json!({ "role": "admin", "disabled": true }),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        // Enabling the other one is fine for the rule, but demoting `admin` in the same
        // request is not: neither part may apply.
        let reply = patch(
            &fx,
            &admin,
            fx.admin,
            json!({ "role": "viewer", "disabled": false }),
        )
        .await;
        assert_eq!(reply.status, StatusCode::CONFLICT);
        let stored = fx.store.user_by_id(fx.admin).await.unwrap().unwrap();
        assert_eq!(stored.role, Role::Admin);
        assert!(!stored.disabled);
        assert_eq!(sessions_of(&fx, fx.admin).await, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn an_admin_may_demote_or_delete_themselves_unless_the_last(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        patch(&fx, &admin, fx.operator, json!({ "role": "admin" })).await;
        let reply = patch(&fx, &admin, fx.admin, json!({ "role": "viewer" })).await;
        assert_eq!(reply.status, StatusCode::OK);
        // The session of the demoted admin ended, like for any other user.
        let me = fx
            .call("GET", "/api/v1/session", Some(&admin), &[], None)
            .await;
        assert_eq!(me.status, StatusCode::UNAUTHORIZED);
        // `operator` is the last admin now: may delete the demoted one, not themselves.
        let second = token_of(&login(&fx, "operator", PASSWORD).await);
        let reply = fx
            .call("DELETE", &url(fx.operator), Some(&second), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::CONFLICT);
        let reply = fx
            .call("DELETE", &url(fx.admin), Some(&second), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        // With a second admin again, deleting oneself works.
        patch(&fx, &second, fx.viewer, json!({ "role": "admin" })).await;
        let reply = fx
            .call("DELETE", &url(fx.operator), Some(&second), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn delete_removes_the_user_and_the_sessions(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        let viewer = fx.session(Role::Viewer).await;
        let reply = fx
            .call("DELETE", &url(fx.viewer), Some(&admin), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        assert!(reply.body.is_empty());
        assert!(fx.store.user_by_id(fx.viewer).await.unwrap().is_none());
        let gone = fx
            .call("GET", "/api/v1/session", Some(&viewer), &[], None)
            .await;
        assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
        let again = fx
            .call("DELETE", &url(fx.viewer), Some(&admin), &[], None)
            .await;
        assert_eq!(again.status, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_id_is_not_found(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        for id in [Uuid::new_v4().to_string(), "not-a-uuid".to_owned()] {
            let path = format!("/api/v1/users/{id}");
            let calls = [
                ("PATCH", path.clone(), Some(r#"{"role":"viewer"}"#)),
                ("POST", format!("{path}/password"), Some("{}")),
                ("DELETE", path.clone(), None),
            ];
            for (method, uri, body) in calls {
                let reply = fx.call(method, &uri, Some(&admin), &[], body).await;
                assert_eq!(reply.status, StatusCode::NOT_FOUND, "{method} {uri}");
                assert_eq!(reply.error_code(), "not_found", "{method} {uri}");
            }
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn no_hash_in_any_response(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let admin = fx.session(Role::Admin).await;
        let replies = vec![
            fx.call("GET", "/api/v1/users", Some(&admin), &[], None)
                .await,
            create(&fx, &admin, json!({ "name": "one", "role": "viewer" })).await,
            create(
                &fx,
                &admin,
                json!({ "name": "two", "role": "viewer", "password": "twelve chars!" }),
            )
            .await,
            patch(&fx, &admin, fx.subject, json!({ "role": "operator" })).await,
            reset(&fx, &admin, fx.subject, "{}").await,
            // Errors too.
            create(&fx, &admin, json!({ "name": "one", "role": "viewer" })).await,
            patch(&fx, &admin, fx.admin, json!({ "disabled": true })).await,
            fx.call("DELETE", &url(fx.subject), Some(&admin), &[], None)
                .await,
            fx.call("GET", "/api/v1/users", Some(&admin), &[], None)
                .await,
        ];
        for reply in replies {
            assert_ne!(reply.status, StatusCode::NOT_FOUND);
            let text = String::from_utf8_lossy(&reply.body).to_lowercase();
            assert!(!text.contains("argon2"), "{text}");
            assert!(!text.contains("password_hash"), "{text}");
        }
    }
}
