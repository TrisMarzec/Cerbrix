//! Users, roles, passwords and the extractors that enforce roles on routes.
//!
//! Passwords are stored only as Argon2id PHC strings; hashing and verifying run on blocking
//! threads because they take tens of milliseconds of CPU and a lot of memory.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher as _, PasswordVerifier as _};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rand::RngExt as _;
use time::OffsetDateTime;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::api::{ApiError, AppState};
use crate::state::{StateStore, User};

/// What a user may do. The order is the order of privilege: `Viewer < Operator < Admin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Reads everything.
    Viewer,
    /// Also acknowledges and closes problems.
    Operator,
    /// Also manages users, enrolment keys, host bindings and display tokens.
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "viewer" => Ok(Role::Viewer),
            "operator" => Ok(Role::Operator),
            "admin" => Ok(Role::Admin),
            other => Err(format!(
                "unknown role {other:?}: expected viewer, operator or admin"
            )),
        }
    }
}

/// Shortest accepted password, in characters.
pub const MIN_PASSWORD_CHARS: usize = 12;
/// Longest accepted password, in characters.
pub const MAX_PASSWORD_CHARS: usize = 256;

/// Checks a user name: `^[a-z0-9][a-z0-9._-]{0,62}$`.
pub fn validate_user_name(s: &str) -> Result<(), String> {
    let bad = || {
        format!(
            "invalid user name {s:?}: use 1 to 63 characters from a-z, 0-9, '.', '_' and '-', \
             starting with a letter or digit"
        )
    };
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return Err(bad());
    }
    let ok_first = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let ok_rest = |b: u8| ok_first(b) || matches!(b, b'.' | b'_' | b'-');
    if ok_first(bytes[0]) && bytes[1..].iter().copied().all(ok_rest) {
        Ok(())
    } else {
        Err(bad())
    }
}

/// Checks the length of a password, counted in characters (Unicode scalar values). A
/// password with a NUL character is refused: PostgreSQL and many tools mishandle it. The
/// message never contains the password.
pub fn validate_password(s: &str) -> Result<(), String> {
    let chars = s.chars().count();
    if chars < MIN_PASSWORD_CHARS {
        return Err(format!(
            "the password must have at least {MIN_PASSWORD_CHARS} characters"
        ));
    }
    if chars > MAX_PASSWORD_CHARS {
        return Err(format!(
            "the password must have at most {MAX_PASSWORD_CHARS} characters"
        ));
    }
    if s.contains('\0') {
        return Err("the password must not contain a NUL character".to_owned());
    }
    Ok(())
}

fn hash_now(password: &str) -> anyhow::Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| anyhow::anyhow!("cannot hash the password: {e}"))
}

fn verify_now(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

/// Hashes a password with Argon2id (default parameters, random salt) into a PHC string, on
/// a blocking thread, without a limit. For the admin command line, a process of its own;
/// the server hashes only through [`PasswordGate`].
pub async fn hash_password(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || hash_now(&password)).await?
}

/// Checks a password against a PHC string, on a blocking thread, without a limit. A
/// malformed hash or a failed worker verifies as false. The server verifies only through
/// [`PasswordGate`].
pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_now(&password, &hash))
        .await
        .unwrap_or(false)
}

/// No permit for a password computation came within the wait: the server is busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

/// Why hashing a password through the gate gave no hash.
#[derive(Debug, thiserror::Error)]
pub enum HashError {
    #[error("the server is busy checking passwords")]
    Busy,
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Bounds the Argon2 work of the server process: every hash and every verification (about
/// 19 MiB and tens of milliseconds of CPU each) needs one of a few permits, so a flood of
/// logins cannot exhaust the memory and CPU that ingestion and alerting need. A caller
/// waits for a permit at most `wait`, then gives up with [`Busy`]. Cheap to clone; the
/// clones share the permits, and the server makes one for the whole process.
#[derive(Debug, Clone)]
pub struct PasswordGate {
    permits: Arc<Semaphore>,
    wait: Duration,
}

impl Default for PasswordGate {
    /// One permit per available core, at least 2 and at most 8; a wait of 5 seconds.
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(2, std::num::NonZero::get);
        Self::new(cores.clamp(2, 8), Duration::from_secs(5))
    }
}

impl PasswordGate {
    /// A gate with `permits` permits and a wait of `wait`. The server uses
    /// [`PasswordGate::default`]; other values are for tests.
    pub fn new(permits: usize, wait: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(permits)),
            wait,
        }
    }

    async fn permit(&self) -> Result<OwnedSemaphorePermit, Busy> {
        match tokio::time::timeout(self.wait, self.permits.clone().acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            // Timed out, or closed (never: the gate does not close its semaphore).
            _ => Err(Busy),
        }
    }

    /// Like [`hash_password`], within the limit. The permit is held by the blocking thread
    /// until the hash is done, even when the caller stops waiting.
    pub async fn hash(&self, password: String) -> Result<String, HashError> {
        let permit = self.permit().await.map_err(|Busy| HashError::Busy)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            hash_now(&password)
        })
        .await
        .map_err(anyhow::Error::from)?
        .map_err(HashError::Failed)
    }

    /// Like [`verify_password`], within the limit. The permit is held by the blocking thread
    /// until the check is done, even when the caller stops waiting.
    pub async fn verify(&self, password: String, hash: String) -> Result<bool, Busy> {
        let permit = self.permit().await?;
        Ok(tokio::task::spawn_blocking(move || {
            let _permit = permit;
            verify_now(&password, &hash)
        })
        .await
        .unwrap_or(false))
    }

    /// Takes a permit and keeps it until the guard is dropped, as a running check would.
    #[cfg(test)]
    pub(crate) fn occupy(&self) -> OwnedSemaphorePermit {
        self.permits
            .clone()
            .try_acquire_owned()
            .expect("a free permit")
    }
}

/// Generates a password of 20 characters, each drawn uniformly from a-z, A-Z and 0-9 with
/// the cryptographically secure thread generator.
pub fn generate_password() -> String {
    rand::rng()
        .sample_iter(rand::distr::Alphanumeric)
        .take(20)
        .map(char::from)
        .collect()
}

/// Name of the session cookie. Its value is the session token and nothing else.
pub const SESSION_COOKIE: &str = "cerbrix_session";

/// How often expired sessions are deleted.
pub const SESSION_CLEANUP_EVERY: Duration = Duration::from_secs(600);

/// The cookie that carries a new session.
pub fn session_cookie(token: &str, max_age_secs: u64, secure: bool) -> String {
    cookie(token, max_age_secs, secure)
}

/// The cookie that removes the session cookie from the browser.
pub fn clear_cookie(secure: bool) -> String {
    cookie("", 0, secure)
}

fn cookie(value: &str, max_age_secs: u64, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!(
        "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age_secs}{secure}"
    )
}

/// The session token of a request: the value of the session cookie, if present.
pub fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, value)| *name == SESSION_COOKIE && !value.is_empty())
        .map(|(_, value)| value)
}

/// Why a request was not let through. Becomes a 401, 403 or 500 answer.
#[derive(Debug)]
pub enum AuthError {
    /// No session, or one that is invalid, expired or belongs to a disabled user. When the
    /// request carried a session cookie, the answer removes it.
    Unauthorized {
        clear_cookie: bool,
    },
    /// A valid session whose role is too low.
    Forbidden,
    Internal,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        match self {
            AuthError::Unauthorized {
                clear_cookie: clear,
            } => {
                let mut response =
                    ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", "login required")
                        .into_response();
                if clear && let Ok(value) = HeaderValue::from_str(&clear_cookie(false)) {
                    response.headers_mut().insert(header::SET_COOKIE, value);
                }
                response
            }
            AuthError::Forbidden => ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "your role does not allow this",
            )
            .into_response(),
            AuthError::Internal => ApiError::internal().into_response(),
        }
    }
}

/// The user a request is made by, put into the request extensions by the route guard so
/// that the extractors below do not look the session up a second time.
#[derive(Debug, Clone)]
pub(crate) struct AuthedUser(pub(crate) User);

/// Resolves the session of a request to its user.
pub(crate) async fn authenticate(app: &AppState, headers: &HeaderMap) -> Result<User, AuthError> {
    let Some(token) = session_token(headers) else {
        return Err(AuthError::Unauthorized {
            clear_cookie: false,
        });
    };
    match app
        .store
        .session_user(token, OffsetDateTime::now_utc(), &app.config.auth)
        .await
    {
        Ok(Some(user)) => Ok(user),
        Ok(None) => Err(AuthError::Unauthorized { clear_cookie: true }),
        Err(e) => {
            tracing::error!(error = %e, "session lookup failed");
            Err(AuthError::Internal)
        }
    }
}

/// `Ok(user)` when the user's role is at least `min`.
pub(crate) fn require(user: User, min: Role) -> Result<User, AuthError> {
    if user.role >= min {
        Ok(user)
    } else {
        Err(AuthError::Forbidden)
    }
}

async fn extract(parts: &mut Parts, app: &AppState, min: Role) -> Result<User, AuthError> {
    let user = match parts.extensions.get::<AuthedUser>() {
        Some(AuthedUser(user)) => user.clone(),
        None => authenticate(app, &parts.headers).await?,
    };
    require(user, min)
}

/// A handler that takes one of these runs only for a session whose user has at least that
/// role: viewer, operator or admin. A request without a valid session is answered 401, one
/// with too low a role 403.
macro_rules! role_extractor {
    ($(#[$doc:meta])* $name:ident, $min:expr) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub struct $name(pub User);

        impl FromRequestParts<AppState> for $name {
            type Rejection = AuthError;

            async fn from_request_parts(
                parts: &mut Parts,
                app: &AppState,
            ) -> Result<Self, Self::Rejection> {
                extract(parts, app, $min).await.map($name)
            }
        }
    };
}

role_extractor!(
    /// Any logged-in user.
    Viewer,
    Role::Viewer
);
role_extractor!(
    /// An operator or an admin.
    Operator,
    Role::Operator
);
role_extractor!(
    /// An admin.
    Admin,
    Role::Admin
);

/// Who may read a display: a logged-in user of any role, or the display token of exactly
/// that display. Only the guard of a `Access::Display` route creates it, so a display token
/// is never accepted by `Viewer`, `Operator` or `Admin` or by any route that is not a
/// display route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayAccess {
    /// The display id of the request path.
    pub display_id: String,
    /// True when a display token let the request in, false for a logged-in user.
    pub by_token: bool,
}

impl FromRequestParts<AppState> for DisplayAccess {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        _app: &AppState,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<DisplayAccess>()
            .cloned()
            .ok_or(AuthError::Unauthorized {
                clear_cookie: false,
            })
    }
}

/// The token of an `Authorization: Bearer <token>` header.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

/// Resolves who calls a display route for the display `display_id`: a logged-in user (who
/// wins when the request carries a session and a token), or else the display token of that
/// display. An unknown or revoked token and the token of another display are the same 401.
pub(crate) async fn authenticate_display(
    app: &AppState,
    headers: &HeaderMap,
    display_id: &str,
) -> Result<(Option<User>, DisplayAccess), AuthError> {
    let mut clear = false;
    if session_token(headers).is_some() {
        match authenticate(app, headers).await {
            Ok(user) => {
                let access = DisplayAccess {
                    display_id: display_id.to_owned(),
                    by_token: false,
                };
                return Ok((Some(user), access));
            }
            Err(AuthError::Unauthorized { clear_cookie }) => clear = clear_cookie,
            Err(e) => return Err(e),
        }
    }
    let unauthorized = AuthError::Unauthorized {
        clear_cookie: clear,
    };
    let Some(token) = bearer_token(headers) else {
        return Err(unauthorized);
    };
    match app.store.display_for_token(token).await {
        Ok(Some(owner)) if owner == display_id => Ok((
            None,
            DisplayAccess {
                display_id: owner,
                by_token: true,
            },
        )),
        Ok(_) => Err(unauthorized),
        Err(e) => {
            tracing::error!(error = %e, "display token lookup failed");
            Err(AuthError::Internal)
        }
    }
}

/// Deletes expired sessions immediately and then every `every` until `shutdown` is
/// cancelled. A failed run is logged and tried again at the next tick.
pub async fn run_session_cleanup_loop(
    store: StateStore,
    every: Duration,
    shutdown: CancellationToken,
) {
    let mut ticker = tokio::time::interval(every.max(Duration::from_millis(1)));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = ticker.tick() => {}
        }
        match store
            .delete_expired_sessions(OffsetDateTime::now_utc())
            .await
        {
            Ok(0) => {}
            Ok(n) => tracing::debug!(sessions = n, "deleted expired sessions"),
            Err(e) => tracing::error!(error = %e, "deleting expired sessions failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "./migrations")]
    async fn expired_sessions_are_deleted_by_the_loop(pool: sqlx::PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let user = store
            .create_user(
                "amy",
                Role::Viewer,
                "$argon2id$x",
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
        let now = OffsetDateTime::now_utc();
        for (name, expires) in [
            ("old", now - time::Duration::hours(1)),
            ("new", now + time::Duration::hours(1)),
        ] {
            sqlx::query(
                "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at) \
                 VALUES ($1, $2, $3, $3, $4)",
            )
            .bind(name.as_bytes())
            .bind(user.id)
            .bind(now - time::Duration::hours(2))
            .bind(expires)
            .execute(&pool)
            .await
            .unwrap();
        }
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(run_session_cleanup_loop(
            store.clone(),
            Duration::from_millis(50),
            shutdown.clone(),
        ));
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let left: Vec<Vec<u8>> = sqlx::query_scalar("SELECT token_hash FROM sessions")
                .fetch_all(&pool)
                .await
                .unwrap();
            if left == [b"new".to_vec()] {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the old session is deleted"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(30), handle)
            .await
            .expect("the loop stops after shutdown")
            .expect("the loop did not panic");
    }

    #[test]
    fn cookie_attributes() {
        assert_eq!(
            session_cookie("cbx_se_abc", 604_800, false),
            "cerbrix_session=cbx_se_abc; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800"
        );
        assert_eq!(
            session_cookie("cbx_se_abc", 60, true),
            "cerbrix_session=cbx_se_abc; Path=/; HttpOnly; SameSite=Strict; Max-Age=60; Secure"
        );
        assert!(clear_cookie(false).starts_with("cerbrix_session=; "));
        assert!(clear_cookie(false).ends_with("Max-Age=0"));
    }

    #[test]
    fn the_token_is_read_from_the_cookie_header() {
        let headers = |values: &[&str]| {
            let mut h = HeaderMap::new();
            for v in values {
                h.append(header::COOKIE, HeaderValue::from_str(v).unwrap());
            }
            h
        };
        assert_eq!(session_token(&headers(&[])), None);
        assert_eq!(session_token(&headers(&["a=b"])), None);
        assert_eq!(session_token(&headers(&["cerbrix_session="])), None);
        assert_eq!(
            session_token(&headers(&["x=1; cerbrix_session=tok; y=2"])),
            Some("tok")
        );
        assert_eq!(
            session_token(&headers(&["x=1", "cerbrix_session=tok"])),
            Some("tok")
        );
        // A cookie that only ends in the name is not the session cookie.
        assert_eq!(session_token(&headers(&["xcerbrix_session=tok"])), None);
    }

    #[test]
    fn role_order_and_text() {
        assert!(Role::Viewer < Role::Operator);
        assert!(Role::Operator < Role::Admin);
        for (text, role) in [
            ("viewer", Role::Viewer),
            ("operator", Role::Operator),
            ("admin", Role::Admin),
        ] {
            assert_eq!(text.parse::<Role>().unwrap(), role);
            assert_eq!(role.as_str(), text);
        }
        assert!("root".parse::<Role>().is_err());
    }

    #[test]
    fn user_name_rules() {
        for ok in ["t.marzec", "ops-1", "a", "0x_y", &"a".repeat(63)] {
            assert!(validate_user_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Admin", "-a", &"a".repeat(64), "a b", "a\n", "ü"] {
            assert!(validate_user_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn password_rules() {
        assert!(validate_password(&"a".repeat(11)).is_err());
        assert!(validate_password(&"a".repeat(12)).is_ok());
        assert!(validate_password(&"a".repeat(256)).is_ok());
        assert!(validate_password(&"a".repeat(257)).is_err());
        // Counted in characters, not bytes.
        assert!(validate_password(&"ü".repeat(12)).is_ok());
        assert!(validate_password(&"ü".repeat(11)).is_err());
        assert!(validate_password(&"ü".repeat(256)).is_ok());
        assert!(validate_password(&"ü".repeat(257)).is_err());
        // NUL is refused; the message does not echo the password.
        let with_nul = format!("{}\0{}", "a".repeat(6), "b".repeat(6));
        let err = validate_password(&with_nul).unwrap_err();
        assert!(err.contains("NUL") && !err.contains("aaaa"), "{err}");
    }

    #[tokio::test]
    async fn hash_and_verify() {
        let password = "correct horse battery".to_owned();
        let a = hash_password(password.clone()).await.unwrap();
        let b = hash_password(password.clone()).await.unwrap();
        assert_ne!(a, b, "the salt is random");
        assert!(a.starts_with("$argon2id$"), "{a}");
        assert!(!a.contains(&password));
        assert!(verify_password(password.clone(), a.clone()).await);
        assert!(verify_password(password.clone(), b).await);
        assert!(!verify_password("wrong horse battery".to_owned(), a).await);
        for malformed in ["", "garbage", "$argon2id$", "$argon2id$v=19$m=1$x$y"] {
            assert!(
                !verify_password(password.clone(), malformed.to_owned()).await,
                "{malformed}"
            );
        }
    }

    #[test]
    fn generated_password_shape() {
        let a = generate_password();
        let b = generate_password();
        assert_eq!(a.chars().count(), 20);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()), "{a}");
        assert_ne!(a, b);
        assert!(validate_password(&a).is_ok());
    }
}
