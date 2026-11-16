//! Login, logout and the session of the logged-in user.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{ApiError, ApiJson, Entry, invalid};
use crate::api::AppState;
use crate::auth::{Viewer, clear_cookie, session_cookie, session_token, validate_password};
use crate::state::{LoginOutcome, PasswordCheck, StateError, User};

pub(super) fn entries() -> Vec<Entry> {
    vec![
        Entry::new("POST", "/api/v1/session", login),
        Entry::new("GET", "/api/v1/session", show),
        Entry::new("DELETE", "/api/v1/session", logout),
        Entry::new("PUT", "/api/v1/session/password", change_password),
    ]
}

#[derive(Deserialize)]
struct LoginBody {
    name: String,
    password: String,
}

#[derive(Serialize)]
struct Me {
    name: String,
    role: &'static str,
}

impl From<&User> for Me {
    fn from(user: &User) -> Self {
        Me {
            name: user.name.clone(),
            role: user.role.as_str(),
        }
    }
}

fn store_failed(what: &str, e: &StateError) -> ApiError {
    tracing::error!(error = %e, "{what}");
    ApiError::internal()
}

fn with_cookie(mut response: Response, cookie: &str) -> Response {
    // The cookie text is built from the token alphabet and fixed attributes.
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

async fn login(
    State(app): State<AppState>,
    ApiJson(body): ApiJson<LoginBody>,
) -> Result<Response, ApiError> {
    let wrong = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "wrong user name or password, or the account is temporarily locked",
        )
    };
    // No user name contains NUL, and PostgreSQL cannot compare with one.
    if body.name.contains('\0') {
        return Err(wrong());
    }
    let outcome = app
        .store
        .login(
            &body.name,
            &body.password,
            OffsetDateTime::now_utc(),
            &app.config.auth,
        )
        .await
        .map_err(|e| store_failed("login failed", &e))?;
    match outcome {
        LoginOutcome::Ok(user, token) => {
            let cookie = session_cookie(
                &token,
                app.config.auth.session_max_secs,
                app.config.secure_cookie(),
            );
            Ok(with_cookie(Json(Me::from(&user)).into_response(), &cookie))
        }
        // A locked account answers exactly like a wrong password: nobody learns which
        // names exist.
        LoginOutcome::Wrong | LoginOutcome::Locked => Err(wrong()),
    }
}

async fn show(Viewer(user): Viewer) -> Json<Me> {
    Json(Me::from(&user))
}

async fn logout(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(token) = session_token(&headers) {
        app.store
            .logout(token)
            .await
            .map_err(|e| store_failed("logout failed", &e))?;
    }
    Ok(with_cookie(
        StatusCode::NO_CONTENT.into_response(),
        &clear_cookie(app.config.secure_cookie()),
    ))
}

#[derive(Deserialize)]
struct PasswordBody {
    current: String,
    new: String,
}

/// Changes the password of the logged-in user and ends the user's other sessions. The
/// current password is checked like a login: a wrong one counts as a failed attempt and a
/// locked user is refused, so a stolen session cannot be used to guess the password.
async fn change_password(
    State(app): State<AppState>,
    Viewer(user): Viewer,
    headers: HeaderMap,
    ApiJson(body): ApiJson<PasswordBody>,
) -> Result<Response, ApiError> {
    // Checked first, and without counting an attempt: it reveals nothing about `current`.
    validate_password(&body.new)
        .map_err(|_| invalid("the new password must have 12 to 256 characters and no NUL"))?;
    let now = OffsetDateTime::now_utc();
    match app
        .store
        .check_password(user.id, &body.current, now)
        .await
        .map_err(|e| store_failed("password check failed", &e))?
    {
        PasswordCheck::Right => {}
        // A wrong password and a locked account answer alike.
        PasswordCheck::Wrong | PasswordCheck::Locked => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "the current password is wrong",
            ));
        }
    }
    let hash = crate::auth::hash_password(body.new).await.map_err(|e| {
        tracing::error!(error = %e, "hashing the new password failed");
        ApiError::internal()
    })?;
    let Some(token) = session_token(&headers) else {
        return Err(ApiError::internal());
    };
    app.store
        .set_user_password_keeping_session(user.id, &hash, now, token)
        .await
        .map_err(|e| store_failed("changing the password failed", &e))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use sqlx::PgPool;

    use crate::api::ui::testing::*;
    use crate::auth::Role;

    fn login_body(name: &str, password: &str) -> String {
        serde_json::json!({ "name": name, "password": password }).to_string()
    }

    async fn login(fx: &Fixture, name: &str, password: &str) -> Reply {
        fx.call(
            "POST",
            "/api/v1/session",
            None,
            &[],
            Some(&login_body(name, password)),
        )
        .await
    }

    /// The token in a `Set-Cookie` header of the session cookie.
    fn token_of(reply: &Reply) -> String {
        let cookie = reply.header("set-cookie").expect("a cookie is set");
        cookie
            .strip_prefix("cerbrix_session=")
            .and_then(|rest| rest.split(';').next())
            .expect("the session cookie")
            .to_owned()
    }

    async fn session_count(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn login_answers_with_name_and_role(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let reply = login(&fx, "operator", PASSWORD).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(
            reply.json(),
            serde_json::json!({ "name": "operator", "role": "operator" })
        );
        // The cookie works for the next request.
        let token = token_of(&reply);
        let me = fx
            .call("GET", "/api/v1/session", Some(&token), &[], None)
            .await;
        assert_eq!(me.status, StatusCode::OK);
        assert_eq!(me.json()["name"], "operator");
        assert_eq!(session_count(&pool).await, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn wrong_login_is_401_and_the_same_for_every_cause(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        sqlx::query("UPDATE users SET disabled = true WHERE name = 'viewer'")
            .execute(&pool)
            .await
            .unwrap();
        let mut seen = Vec::new();
        for (name, password) in [
            ("admin", "not the password"),
            ("nobody", PASSWORD),
            ("viewer", PASSWORD), // disabled
            ("bad\0name", PASSWORD),
        ] {
            let reply = login(&fx, name, password).await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{name}");
            assert!(reply.header("set-cookie").is_none(), "{name}");
            seen.push(reply.body);
        }
        assert!(seen.windows(2).all(|w| w[0] == w[1]), "identical answers");
        let body: serde_json::Value = serde_json::from_slice(&seen[0]).unwrap();
        assert_eq!(body["error"], "unauthorized");
        assert_eq!(
            body["message"],
            "wrong user name or password, or the account is temporarily locked"
        );
        assert_eq!(session_count(&pool).await, 0);
    }

    fn comparable(reply: &Reply) -> (StatusCode, Vec<u8>, Vec<(String, String)>) {
        let mut headers: Vec<_> = reply
            .headers
            .iter()
            .filter(|(name, _)| *name != "date")
            .map(|(n, v)| (n.to_string(), v.to_str().unwrap().to_owned()))
            .collect();
        headers.sort();
        (reply.status, reply.body.clone(), headers)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn locked_and_unknown_user_answer_identically(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        for _ in 0..5 {
            assert_eq!(
                login(&fx, "admin", "wrong").await.status,
                StatusCode::UNAUTHORIZED
            );
        }
        // Locked, even for the right password: the answer is that of an unknown name.
        let locked = login(&fx, "admin", PASSWORD).await;
        let unknown = login(&fx, "nobody", PASSWORD).await;
        assert_eq!(locked.status, StatusCode::UNAUTHORIZED);
        assert_eq!(comparable(&locked), comparable(&unknown));
        assert_eq!(locked.error_code(), "unauthorized");
        assert_eq!(
            locked.json()["message"],
            "wrong user name or password, or the account is temporarily locked"
        );
        assert!(locked.header("set-cookie").is_none());
        // Other users are not affected.
        assert_eq!(login(&fx, "viewer", PASSWORD).await.status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn malformed_login_bodies_are_invalid(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        for body in [
            "",
            "{}",
            r#"{"name":"admin"}"#,
            r#"{"name":1,"password":2}"#,
            "[]",
            "nope",
        ] {
            let reply = fx
                .call("POST", "/api/v1/session", None, &[], Some(body))
                .await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body:?}");
            assert_eq!(reply.error_code(), "invalid", "{body:?}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn login_sets_the_cookie_with_its_attributes(pool: PgPool) {
        // (insecure_http, auth.secure_cookie, expected Secure)
        for (insecure_http, secure_cookie, secure) in [
            (true, None, false),
            (false, None, true),
            (true, Some(true), true),
            (false, Some(false), false),
        ] {
            let pool = {
                // A fresh database per case: the fixture creates the same user names.
                sqlx::query("DELETE FROM users")
                    .execute(&pool)
                    .await
                    .unwrap();
                pool.clone()
            };
            let fx = Fixture::with_config(pool, config(insecure_http, secure_cookie)).await;
            let reply = login(&fx, "viewer", PASSWORD).await;
            assert_eq!(reply.status, StatusCode::OK);
            let cookie = reply.header("set-cookie").unwrap();
            let token = token_of(&reply);
            assert!(token.starts_with("cbx_se_"), "{cookie}");
            assert_eq!(
                cookie,
                format!(
                    "cerbrix_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800{}",
                    if secure { "; Secure" } else { "" }
                ),
                "insecure_http={insecure_http} secure_cookie={secure_cookie:?}"
            );
            // The cookie holds the token and nothing else; the body does not repeat it.
            assert!(!String::from_utf8_lossy(&reply.body).contains(&token));
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn session_endpoint_needs_a_session(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let reply = fx.call("GET", "/api/v1/session", None, &[], None).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert_eq!(reply.error_code(), "unauthorized");
        for role in [Role::Viewer, Role::Operator, Role::Admin] {
            let token = fx.session(role).await;
            let reply = fx
                .call("GET", "/api/v1/session", Some(&token), &[], None)
                .await;
            assert_eq!(reply.status, StatusCode::OK);
            assert_eq!(
                reply.json(),
                serde_json::json!({ "name": role.as_str(), "role": role.as_str() })
            );
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn logout_clears_cookie_and_session(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let token = token_of(&login(&fx, "viewer", PASSWORD).await);
        let other = token_of(&login(&fx, "viewer", PASSWORD).await);
        let reply = fx
            .call("DELETE", "/api/v1/session", Some(&token), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        assert!(reply.body.is_empty());
        let cookie = reply.header("set-cookie").unwrap();
        assert!(cookie.starts_with("cerbrix_session=;"), "{cookie}");
        assert!(cookie.contains("Max-Age=0"), "{cookie}");
        assert_eq!(session_count(&pool).await, 1);
        let again = fx
            .call("GET", "/api/v1/session", Some(&token), &[], None)
            .await;
        assert_eq!(again.status, StatusCode::UNAUTHORIZED);
        // The user's other session is not affected.
        let still = fx
            .call("GET", "/api/v1/session", Some(&other), &[], None)
            .await;
        assert_eq!(still.status, StatusCode::OK);
    }

    fn password_body(current: &str, new: &str) -> String {
        serde_json::json!({ "current": current, "new": new }).to_string()
    }

    async fn change(fx: &Fixture, token: &str, current: &str, new: &str) -> Reply {
        fx.call(
            "PUT",
            "/api/v1/session/password",
            Some(token),
            &[],
            Some(&password_body(current, new)),
        )
        .await
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn change_own_password(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let current = token_of(&login(&fx, "operator", PASSWORD).await);
        let second = token_of(&login(&fx, "operator", PASSWORD).await);
        let new = "a brand new password";
        let reply = change(&fx, &current, PASSWORD, new).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT, "{:?}", reply.json());
        // The old password no longer logs in, the new one does.
        assert_eq!(
            login(&fx, "operator", PASSWORD).await.status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(login(&fx, "operator", new).await.status, StatusCode::OK);
        // The other session ended, this one stays.
        let gone = fx
            .call("GET", "/api/v1/session", Some(&second), &[], None)
            .await;
        assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
        let stays = fx
            .call("GET", "/api/v1/session", Some(&current), &[], None)
            .await;
        assert_eq!(stays.status, StatusCode::OK);
        // Another user's sessions are untouched.
        let admin = fx.session(Role::Admin).await;
        assert_eq!(
            change(&fx, &admin, PASSWORD, new).await.status,
            StatusCode::NO_CONTENT
        );
        let viewer = fx.session(Role::Viewer).await;
        let ok = fx
            .call("GET", "/api/v1/session", Some(&viewer), &[], None)
            .await;
        assert_eq!(ok.status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn change_password_refuses_wrong_current_and_invalid_new(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let token = fx.session(Role::Operator).await;
        // A new password that is too short: 400, and it does not count as an attempt.
        let reply = change(&fx, &token, "wrong", "short").await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(reply.error_code(), "invalid");
        assert!(!String::from_utf8_lossy(&reply.body).contains("short\""));
        let failed: i32 =
            sqlx::query_scalar("SELECT failed_logins FROM users WHERE name = 'operator'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(failed, 0);
        // A wrong current password: 403, and it counts like a failed login.
        let reply = change(&fx, &token, "wrong", "a brand new password").await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert_eq!(reply.error_code(), "forbidden");
        let failed: i32 =
            sqlx::query_scalar("SELECT failed_logins FROM users WHERE name = 'operator'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(failed, 1);
        // The password did not change.
        assert_eq!(
            login(&fx, "operator", PASSWORD).await.status,
            StatusCode::OK
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_stolen_session_cannot_guess_the_password(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let token = fx.session(Role::Admin).await;
        for _ in 0..5 {
            let reply = change(&fx, &token, "wrong", "a brand new password").await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
        }
        // Locked now: even the right current password is refused with the same 403, and
        // so is a login (as a wrong one).
        let reply = change(&fx, &token, PASSWORD, "a brand new password").await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert_eq!(reply.error_code(), "forbidden");
        assert_eq!(
            login(&fx, "admin", PASSWORD).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
}
