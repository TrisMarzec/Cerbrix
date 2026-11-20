//! Login, logout and the session of the logged-in user.

use std::net::SocketAddr;
use std::time::Instant;

use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{
    ApiError, ApiJson, Entry, hash_failed, invalid, logins_refused, passwords_busy, store_failed,
};
use crate::api::AppState;
use crate::auth::throttle::client_address;
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

fn with_cookie(mut response: Response, cookie: &str) -> Response {
    // The cookie text is built from the token alphabet and fixed attributes.
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

async fn login(
    State(app): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<LoginBody>,
) -> Result<Response, ApiError> {
    let client = client_address(peer.ip(), &headers, app.client_address_header.as_ref());
    // Before any password work and before the account's own counter, and the same for
    // every name.
    if app.logins.refused(client, Instant::now()) {
        return Err(logins_refused());
    }
    // A failed login, counted against the client address.
    let wrong = || {
        app.logins.failed(client, Instant::now());
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "wrong user name or password, or the account is temporarily locked",
        )
    };
    // No user name contains NUL, and PostgreSQL cannot compare with one. Counted like an
    // unknown name.
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
            &app.passwords,
        )
        .await
        .map_err(|e| store_failed("login failed", &e))?;
    match outcome {
        LoginOutcome::Ok(user, token) => Ok(with_session(
            &app,
            Json(Me::from(&user)).into_response(),
            &token,
        )),
        // A locked account answers exactly like a wrong password: nobody learns which
        // names exist. Both count against the client address.
        LoginOutcome::Wrong | LoginOutcome::Locked => Err(wrong()),
        // The same for every name, and nothing was counted.
        LoginOutcome::Busy => Err(passwords_busy()),
    }
}

/// `response` with the cookie of the new session `token`.
fn with_session(app: &AppState, response: Response, token: &str) -> Response {
    let cookie = session_cookie(
        token,
        app.config.auth.session_max_secs,
        app.config.secure_cookie(),
    );
    with_cookie(response, &cookie)
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

/// Changes the password of the logged-in user, ends all of the user's sessions and starts a
/// new one for the caller, whose cookie the answer sets: a copy of the old cookie does not
/// survive the change. The current password is checked like a login: a wrong one counts as
/// a failed attempt and a locked user is refused, so a stolen session cannot be used to
/// guess the password.
async fn change_password(
    State(app): State<AppState>,
    Viewer(user): Viewer,
    ApiJson(body): ApiJson<PasswordBody>,
) -> Result<Response, ApiError> {
    // Checked first, and without counting an attempt: it reveals nothing about `current`.
    validate_password(&body.new)
        .map_err(|_| invalid("the new password must have 12 to 256 characters and no NUL"))?;
    let now = OffsetDateTime::now_utc();
    let verified = match app
        .store
        .check_password(user.id, &body.current, now, &app.passwords)
        .await
        .map_err(|e| store_failed("password check failed", &e))?
    {
        PasswordCheck::Right { hash } => hash,
        // A wrong password and a locked account answer alike.
        PasswordCheck::Wrong | PasswordCheck::Locked => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "the current password is wrong",
            ));
        }
        PasswordCheck::Busy => return Err(passwords_busy()),
    };
    let hash = app.passwords.hash(body.new).await.map_err(hash_failed)?;
    // Stored only while the password is still the verified one: a reset by an admin while
    // this request waited for the hashing wins.
    let token = app
        .store
        .set_user_password_renewing_session(user.id, &verified, &hash, now, &app.config.auth)
        .await
        .map_err(|e| change_error(&e))?;
    Ok(with_session(
        &app,
        StatusCode::NO_CONTENT.into_response(),
        &token,
    ))
}

/// The answer when storing a changed password failed.
fn change_error(e: &StateError) -> ApiError {
    match e {
        StateError::Conflict(_) => ApiError::new(
            StatusCode::CONFLICT,
            "conflict",
            "the password was changed in the meantime",
        ),
        _ => store_failed("changing the password failed", e),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use sqlx::PgPool;

    use crate::api::ui::testing::*;
    use crate::auth::{PasswordGate, Role};
    use crate::state::StateError;

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
        assert!(reply.body.is_empty());
        // The answer carries a fresh session, with the attributes of a login.
        let renewed = token_of(&reply);
        assert!(renewed.starts_with("cbx_se_"));
        assert_ne!(renewed, current);
        assert_eq!(
            reply.header("set-cookie").unwrap(),
            format!("cerbrix_session={renewed}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800")
        );
        let (created, expires): (time::OffsetDateTime, time::OffsetDateTime) =
            sqlx::query_as("SELECT created_at, expires_at FROM sessions WHERE token_hash = $1")
                .bind(&crate::tokens::hash(&renewed)[..])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(expires - created, time::Duration::seconds(604_800));
        // The old password no longer logs in, the new one does.
        assert_eq!(
            login(&fx, "operator", PASSWORD).await.status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(login(&fx, "operator", new).await.status, StatusCode::OK);
        // The other session and the old cookie of this one ended; the new cookie works.
        for ended in [&second, &current] {
            let gone = fx
                .call("GET", "/api/v1/session", Some(ended), &[], None)
                .await;
            assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
        }
        let works = fx
            .call("GET", "/api/v1/session", Some(&renewed), &[], None)
            .await;
        assert_eq!(works.status, StatusCode::OK);
        assert_eq!(works.json()["name"], "operator");
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

    async fn failed_logins(pool: &PgPool, name: &str) -> i32 {
        sqlx::query_scalar("SELECT failed_logins FROM users WHERE name = $1")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// A fixture whose password checks wait at most `wait` for the one permit there is.
    fn gate(fx: &mut Fixture, wait: std::time::Duration) -> PasswordGate {
        let gate = PasswordGate::new(1, wait);
        fx.use_password_gate(gate.clone());
        gate
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_busy_server_answers_429_alike_and_counts_nothing(pool: PgPool) {
        let mut fx = Fixture::new(pool.clone()).await;
        // Lock the admin: five wrong passwords.
        for _ in 0..5 {
            login(&fx, "admin", "wrong").await;
        }
        let wait = std::time::Duration::from_millis(200);
        let gate = gate(&mut fx, wait);
        let held = gate.occupy();
        let mut answers = Vec::new();
        for (name, password) in [
            ("operator", PASSWORD),   // exists, right password
            ("operator", "wrong pw"), // exists, wrong password
            ("nobody", PASSWORD),     // unknown
            ("admin", PASSWORD),      // locked
        ] {
            let started = std::time::Instant::now();
            let reply = login(&fx, name, password).await;
            let took = started.elapsed();
            assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS, "{name}");
            assert_eq!(reply.error_code(), "too_many_attempts", "{name}");
            assert_eq!(
                reply.json()["message"],
                "the server is busy checking passwords, try again shortly"
            );
            assert!(reply.header("set-cookie").is_none(), "{name}");
            // It waited for the permit, and not much longer than that.
            assert!(
                took >= wait && took < std::time::Duration::from_secs(30),
                "{took:?}"
            );
            answers.push(comparable(&reply));
        }
        assert!(
            answers.windows(2).all(|w| w[0] == w[1]),
            "identical answers"
        );
        // A waiting request learned nothing and counts as no attempt.
        assert_eq!(failed_logins(&pool, "operator").await, 0);
        assert_eq!(failed_logins(&pool, "admin").await, 5);
        assert_eq!(session_count(&pool).await, 0);
        // With the permit free, the login proceeds.
        drop(held);
        assert_eq!(
            login(&fx, "operator", PASSWORD).await.status,
            StatusCode::OK
        );
        assert_eq!(
            login(&fx, "operator", "wrong pw").await.status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(failed_logins(&pool, "operator").await, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_login_waits_for_a_permit_that_comes_free(pool: PgPool) {
        let mut fx = Fixture::new(pool).await;
        // A generous wait: the permit is given back long before it ends.
        let gate = gate(&mut fx, std::time::Duration::from_secs(30));
        let held = gate.occupy();
        let release = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            drop(held);
        });
        assert_eq!(login(&fx, "viewer", PASSWORD).await.status, StatusCode::OK);
        release.await.unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn every_route_that_hashes_answers_429_when_busy(pool: PgPool) {
        let mut fx = Fixture::new(pool.clone()).await;
        let gate = gate(&mut fx, std::time::Duration::from_millis(100));
        let operator = fx.session(Role::Operator).await;
        let admin = fx.session(Role::Admin).await;
        let held = gate.occupy();
        let reset = format!("/api/v1/users/{}/password", fx.subject);
        for (method, uri, token, body) in [
            (
                "PUT",
                "/api/v1/session/password",
                &operator,
                password_body(PASSWORD, "a brand new password"),
            ),
            (
                "POST",
                "/api/v1/users",
                &admin,
                r#"{"name": "newbie", "role": "viewer"}"#.to_owned(),
            ),
            ("POST", reset.as_str(), &admin, "{}".to_owned()),
        ] {
            let reply = fx.call(method, uri, Some(token), &[], Some(&body)).await;
            assert_eq!(
                reply.status,
                StatusCode::TOO_MANY_REQUESTS,
                "{method} {uri}"
            );
            assert_eq!(reply.error_code(), "too_many_attempts", "{method} {uri}");
        }
        // Nothing changed: no user, no new password, no counted attempt, sessions kept.
        assert_eq!(failed_logins(&pool, "operator").await, 0);
        let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE name = 'newbie'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(users, 0);
        drop(held);
        assert_eq!(
            login(&fx, "operator", PASSWORD).await.status,
            StatusCode::OK
        );
        assert_eq!(login(&fx, "subject", PASSWORD).await.status, StatusCode::OK);
    }

    /// A login from the client address `peer` with extra request `headers`.
    async fn login_from(
        fx: &Fixture,
        peer: &str,
        headers: &[(&str, &str)],
        name: &str,
        password: &str,
    ) -> Reply {
        fx.call_from(
            peer.parse().unwrap(),
            "POST",
            "/api/v1/session",
            headers,
            Some(&login_body(name, password)),
        )
        .await
    }

    /// A 429 `too_many_attempts` with `message` and no cookie.
    fn assert_429(reply: &Reply, message: &str) {
        assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(reply.error_code(), "too_many_attempts");
        assert_eq!(reply.json()["message"], message);
        assert!(reply.header("set-cookie").is_none());
    }

    /// The answer to a refused client address.
    fn assert_refused(reply: &Reply) {
        assert_429(
            reply,
            "too many failed logins from this address; try again in a few minutes",
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn eleventh_failed_login_from_an_address_is_429(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        for n in 1..=10 {
            let reply = login(&fx, "nobody", PASSWORD).await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "attempt {n}");
        }
        assert_refused(&login(&fx, "nobody", PASSWORD).await);
        // A right password is refused too: nothing is checked.
        assert_refused(&login(&fx, "viewer", PASSWORD).await);
        let other = login_from(&fx, "192.0.2.7", &[], "nobody", PASSWORD).await;
        assert_eq!(other.status, StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_refused_address_does_not_count_against_the_account(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let a = "198.51.100.1";
        for _ in 0..4 {
            let reply = login_from(&fx, a, &[], "admin", "wrong").await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        }
        assert_eq!(failed_logins(&pool, "admin").await, 4);
        for _ in 4..10 {
            let reply = login_from(&fx, a, &[], "nobody", "wrong").await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        }
        // Refused before the account's own counter: the admin is not locked by them.
        for _ in 0..3 {
            assert_refused(&login_from(&fx, a, &[], "admin", "wrong").await);
        }
        assert_eq!(failed_logins(&pool, "admin").await, 4);
        let b = login_from(&fx, "198.51.100.2", &[], "admin", PASSWORD).await;
        assert_eq!(b.status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_successful_login_is_not_counted(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        for _ in 0..9 {
            login(&fx, "nobody", PASSWORD).await;
        }
        for _ in 0..3 {
            assert_eq!(login(&fx, "viewer", PASSWORD).await.status, StatusCode::OK);
        }
        // The tenth failure is still answered, the eleventh attempt is refused.
        assert_eq!(
            login(&fx, "nobody", PASSWORD).await.status,
            StatusCode::UNAUTHORIZED
        );
        assert_refused(&login(&fx, "viewer", PASSWORD).await);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_busy_server_is_not_counted(pool: PgPool) {
        let mut fx = Fixture::new(pool).await;
        let gate = gate(&mut fx, std::time::Duration::from_millis(20));
        let held = gate.occupy();
        for _ in 0..12 {
            assert_429(
                &login(&fx, "nobody", PASSWORD).await,
                "the server is busy checking passwords, try again shortly",
            );
        }
        drop(held);
        for n in 1..=10 {
            let reply = login(&fx, "nobody", PASSWORD).await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "attempt {n}");
        }
        assert_refused(&login(&fx, "nobody", PASSWORD).await);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn the_configured_header_chooses_the_address(pool: PgPool) {
        let mut config = (*config(true, None)).clone();
        config.auth.client_address_header = Some("x-forwarded-for".to_owned());
        let fx = Fixture::with_config(pool, std::sync::Arc::new(config)).await;
        let proxy = "10.0.0.2";
        let from = |addr: &'static str| [("x-forwarded-for", addr)];
        for _ in 0..10 {
            let reply = login_from(&fx, proxy, &from("203.0.113.5"), "nobody", PASSWORD).await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        }
        assert_refused(&login_from(&fx, proxy, &from("203.0.113.5"), "nobody", PASSWORD).await);
        // Another client behind the same proxy, and the proxy itself, are not refused.
        let other = login_from(&fx, proxy, &from("203.0.113.6"), "nobody", PASSWORD).await;
        assert_eq!(other.status, StatusCode::UNAUTHORIZED);
        let direct = login_from(&fx, proxy, &[], "nobody", PASSWORD).await;
        assert_eq!(direct.status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn a_password_changed_in_the_meantime_is_409() {
        let e = super::change_error(&StateError::Conflict(
            "the password was changed in the meantime".to_owned(),
        ));
        assert_eq!(
            (e.status, e.code, &*e.message),
            (
                StatusCode::CONFLICT,
                "conflict",
                "the password was changed in the meantime"
            )
        );
        // Other failures answer like every failed store call.
        let e = super::change_error(&StateError::NotFound);
        assert_eq!(e.status, StatusCode::INTERNAL_SERVER_ERROR);
        let e = super::change_error(&StateError::Db(sqlx::Error::PoolTimedOut));
        assert_eq!(
            (e.status, e.code),
            (StatusCode::SERVICE_UNAVAILABLE, "database_unavailable")
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
