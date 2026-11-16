//! The API of the web UI under `/api/v1`.
//!
//! Every route is listed in [`ROUTES`] with the role it needs. [`router`] registers a route
//! only through [`Entry`] and looks its access up in `ROUTES`; the guard it puts around the
//! route enforces that role before the handler runs, whether or not the handler asks for
//! the user. A route missing from `ROUTES` stops the router from being built. Handlers that
//! need the user take one of the extractors `Viewer`, `Operator` or `Admin`, which check
//! the role again; the `access_matrix` test fails when the two disagree.

use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Query, Request, State};
use axum::handler::Handler;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, delete, get, post, put};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{ApiError, AppState};
use crate::auth::{AuthError, AuthedUser, Role, authenticate, require};

mod session;

/// Largest request body of a UI route.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// Who may call a route. `Operator`, `Admin` and `Display` are used by the routes of later
/// tasks.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Anyone; only the login.
    Public,
    Viewer,
    Operator,
    Admin,
    /// A display token (arrives with the display endpoints). Until then the guard refuses
    /// every request to such a route.
    Display,
}

impl Access {
    /// The role a session needs, `None` for the other kinds.
    fn role(self) -> Option<Role> {
        match self {
            Access::Viewer => Some(Role::Viewer),
            Access::Operator => Some(Role::Operator),
            Access::Admin => Some(Role::Admin),
            Access::Public | Access::Display => None,
        }
    }
}

/// Every route of the UI API: method, path pattern and access. One line per route; the
/// router is built from the handlers registered in `entries` and checked against this list.
pub(crate) const ROUTES: &[(&str, &str, Access)] = &[
    ("POST", "/api/v1/session", Access::Public),
    ("GET", "/api/v1/session", Access::Viewer),
    ("DELETE", "/api/v1/session", Access::Viewer),
    ("PUT", "/api/v1/session/password", Access::Viewer),
];

/// One registered route: the handler for a method and path. Built with `Entry::new`, the
/// only way to register a route.
pub(crate) struct Entry {
    method: &'static str,
    path: &'static str,
    route: MethodRouter<AppState>,
}

impl Entry {
    pub(crate) fn new<H, T>(method: &'static str, path: &'static str, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        let route = match method {
            "GET" => get(handler),
            "POST" => post(handler),
            "PUT" => put(handler),
            "DELETE" => delete(handler),
            other => panic!("unsupported method {other} for {path}"),
        };
        Self {
            method,
            path,
            route,
        }
    }
}

/// All handlers, from every module of the UI API.
fn entries() -> Vec<Entry> {
    let mut all = Vec::new();
    all.extend(session::entries());
    all
}

/// The access `ROUTES` gives a registered route.
///
/// # Panics
///
/// When the route is not in `ROUTES`: a route without a stated access must not be served.
fn access_of(entry: &Entry) -> Access {
    ROUTES
        .iter()
        .find(|(m, p, _)| *m == entry.method && *p == entry.path)
        .map(|(_, _, access)| *access)
        .unwrap_or_else(|| {
            panic!(
                "route {} {} is registered but missing from ROUTES",
                entry.method, entry.path
            )
        })
}

/// What the guard of one route needs to decide.
#[derive(Clone)]
struct Guard {
    app: AppState,
    access: Access,
}

/// Lets a request through only when its access level is met, and hands the resolved user to
/// the handler's extractors.
async fn guard(State(guard): State<Guard>, mut req: Request, next: Next) -> Response {
    let user = match guard.access {
        Access::Public => return next.run(req).await,
        // No display endpoint exists yet: fail closed.
        Access::Display => {
            return AuthError::Unauthorized {
                clear_cookie: false,
            }
            .into_response();
        }
        Access::Viewer | Access::Operator | Access::Admin => {
            let min = guard.access.role().unwrap_or(Role::Admin);
            match authenticate(&guard.app, req.headers())
                .await
                .and_then(|user| require(user, min))
            {
                Ok(user) => user,
                Err(e) => return e.into_response(),
            }
        }
    };
    req.extensions_mut().insert(AuthedUser(user));
    next.run(req).await
}

/// Everything under `/api/v1`. Merged into `api::router`, so one server serves both APIs.
///
/// # Panics
///
/// When a handler is registered for a route that is not in `ROUTES`.
pub fn router(state: AppState) -> Router {
    let mut router = Router::new();
    for entry in entries() {
        let access = access_of(&entry);
        let guarded = entry.route.route_layer(from_fn_with_state(
            Guard {
                app: state.clone(),
                access,
            },
            guard,
        ));
        router = router.route(entry.path, guarded);
    }
    router
        .with_state(state)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(from_fn(request_checks))
        .layer(from_fn(no_store))
}

/// Answers must not be cached: they depend on the session.
async fn no_store(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn invalid(message: &'static str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid", message)
}

fn forbidden(message: &'static str) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "forbidden", message)
}

/// The checks every request goes through before routing to a handler: the same-origin rule
/// for requests that change something, and the content type of a body.
async fn request_checks(req: Request, next: Next) -> Response {
    if !matches!(*req.method(), Method::GET | Method::HEAD)
        && let Err(e) = same_origin(&req)
    {
        return e.into_response();
    }
    if !axum::body::HttpBody::is_end_stream(req.body()) && !is_json(req.headers()) {
        return invalid("the body must be JSON with Content-Type: application/json")
            .into_response();
    }
    next.run(req).await
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            let essence = v.split(';').next().unwrap_or("").trim();
            essence.eq_ignore_ascii_case("application/json")
        })
}

/// The same-origin rule: when `Origin` is present, its host and port must be those of the
/// `Host` header (the scheme is not compared: a TLS-terminating reverse proxy changes it);
/// otherwise a present `Sec-Fetch-Site` must be `same-origin`; a request with neither does
/// not come from a browser page and is let through.
fn same_origin(req: &Request) -> Result<(), ApiError> {
    const MESSAGE: &str = "cross-origin requests are not allowed";
    let headers = req.headers();
    if let Some(origin) = headers.get(header::ORIGIN) {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .or_else(|| req.uri().authority().map(|a| a.as_str().to_owned()));
        let same = origin
            .to_str()
            .ok()
            .zip(host)
            .is_some_and(|(origin, host)| origin_matches_host(origin, &host));
        return if same {
            Ok(())
        } else {
            Err(forbidden(MESSAGE))
        };
    }
    match headers.get("sec-fetch-site") {
        None => Ok(()),
        Some(v) if v.as_bytes() == b"same-origin" => Ok(()),
        Some(_) => Err(forbidden(MESSAGE)),
    }
}

/// True when `origin` (`scheme://host[:port]`) names the server in `host` (`host[:port]`),
/// ignoring the case of the host and treating the default port of the origin's scheme as no
/// port. `null`, a malformed origin or one with a path, user name or query is not a match.
fn origin_matches_host(origin: &str, host: &str) -> bool {
    let Ok(origin) = url::Url::parse(origin) else {
        return false;
    };
    let plain = matches!(origin.scheme(), "http" | "https")
        && origin.path() == "/"
        && origin.query().is_none()
        && origin.fragment().is_none()
        && origin.username().is_empty()
        && origin.password().is_none();
    let Some(origin_host) = origin.host_str().filter(|_| plain) else {
        return false;
    };
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    // `Url::port` is `None` for the default port of the scheme. A `Host` without a port
    // means that default too, and a `Host` with the default port equals an origin without.
    let same_port = match authority.port_u16() {
        None => origin.port().is_none(),
        Some(port) => origin.port_or_known_default() == Some(port),
    };
    same_port && origin_host.eq_ignore_ascii_case(authority.host())
}

/// A JSON request body. Unlike `axum::Json`, a rejection is an `ApiError` in the shape of
/// every other error: 400 `invalid`, and 413 for a body over the limit.
pub(crate) struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request<Body>, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ApiJson(value)),
            Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "invalid",
                    "the request body is larger than 64 KiB",
                ))
            }
            Err(_) => Err(invalid("the body is not valid JSON of the expected shape")),
        }
    }
}

/// Paging of a list: `?limit=` (1 to 500, default 50) and `?offset=` (from 0, default 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

#[derive(Deserialize)]
struct RawPage {
    limit: Option<i64>,
    offset: Option<i64>,
}

impl<S: Send + Sync> FromRequestParts<S> for Page {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(raw) = Query::<RawPage>::from_request_parts(parts, state)
            .await
            .map_err(|_| invalid("limit and offset must be whole numbers"))?;
        let limit = raw.limit.unwrap_or(50);
        let offset = raw.offset.unwrap_or(0);
        if !(1..=500).contains(&limit) {
            return Err(invalid("limit must be between 1 and 500"));
        }
        if offset < 0 {
            return Err(invalid("offset must not be negative"));
        }
        Ok(Page { limit, offset })
    }
}

/// The answer of every list: one page of items and the number of all matching items.
#[derive(Debug, Serialize)]
pub struct Listed<T> {
    pub items: Vec<T>,
    pub total: i64,
}

#[cfg(test)]
pub(crate) mod testing {
    //! A fixture for the tests of the UI routes. Later tasks add path parameters to
    //! `Fixture::fill` and, when a route needs more data, to `Fixture::new`.

    use std::net::SocketAddr;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt as _;
    use sqlx::PgPool;
    use time::{Duration, OffsetDateTime};
    use tower::ServiceExt as _;
    use uuid::Uuid;

    use super::*;
    use crate::auth::{Role, hash_password};
    use crate::config::{
        AuthConfig, Config, DatabaseConfig, IngestConfig, MetricsConfig, ServerConfig,
    };
    use crate::metrics::MemoryMetrics;
    use crate::state::StateStore;
    use crate::tokens;

    /// The password of every fixture user.
    pub const PASSWORD: &str = "correct horse battery";

    pub fn config(insecure_http: bool, secure_cookie: Option<bool>) -> Arc<Config> {
        Arc::new(Config {
            server: ServerConfig {
                listen: SocketAddr::from(([127, 0, 0, 1], 0)),
                tls_cert: None,
                tls_key: None,
                insecure_http,
            },
            database: DatabaseConfig {
                url: "postgres://localhost/unused".into(),
            },
            metrics: MetricsConfig {
                url: "http://localhost:8428".into(),
            },
            ingest: IngestConfig::default(),
            gitops: Default::default(),
            alerting: Default::default(),
            smtp: Default::default(),
            auth: AuthConfig {
                secure_cookie,
                ..AuthConfig::default()
            },
        })
    }

    pub struct Reply {
        pub status: StatusCode,
        pub headers: axum::http::HeaderMap,
        pub body: Vec<u8>,
    }

    impl Reply {
        pub fn json(&self) -> serde_json::Value {
            serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
        }

        pub fn error_code(&self) -> String {
            self.json()["error"].as_str().unwrap_or("").to_owned()
        }

        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers.get(name).and_then(|v| v.to_str().ok())
        }
    }

    pub struct Fixture {
        pub router: Router,
        #[allow(dead_code)] // for the tests of later tasks
        pub store: StateStore,
        pub pool: PgPool,
        /// The fixture users, one per role.
        pub viewer: Uuid,
        pub operator: Uuid,
        pub admin: Uuid,
    }

    impl Fixture {
        pub async fn new(pool: PgPool) -> Self {
            Self::with_config(pool, config(true, None)).await
        }

        pub async fn with_config(pool: PgPool, config: Arc<Config>) -> Self {
            let store = StateStore::from_pool(pool.clone());
            let hash = hash_password(PASSWORD.to_owned()).await.unwrap();
            let now = OffsetDateTime::now_utc();
            let mut ids = Vec::new();
            for (name, role) in [
                ("viewer", Role::Viewer),
                ("operator", Role::Operator),
                ("admin", Role::Admin),
            ] {
                ids.push(store.create_user(name, role, &hash, now).await.unwrap().id);
            }
            let router = crate::api::router(crate::api::AppState {
                store: store.clone(),
                metrics: Arc::new(MemoryMetrics::new()),
                config,
                warnings: Arc::default(),
                queue_mail: false,
            });
            Self {
                router,
                store,
                pool,
                viewer: ids[0],
                operator: ids[1],
                admin: ids[2],
            }
        }

        /// A new session of the fixture user with `role`; returns its token.
        pub async fn session(&self, role: Role) -> String {
            let user = match role {
                Role::Viewer => self.viewer,
                Role::Operator => self.operator,
                Role::Admin => self.admin,
            };
            let token = tokens::generate("cbx_se_");
            let now = OffsetDateTime::now_utc();
            sqlx::query(
                "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at) \
                 VALUES ($1, $2, $3, $3, $4)",
            )
            .bind(&tokens::hash(&token)[..])
            .bind(user)
            .bind(now)
            .bind(now + Duration::hours(1))
            .execute(&self.pool)
            .await
            .unwrap();
            token
        }

        /// Fills the path parameters of a route pattern with ids that exist in the fixture.
        /// A later task that adds a route with a parameter adds the parameter here.
        pub fn fill(&self, pattern: &str) -> String {
            pattern.to_owned()
        }

        /// Sends a request. `cookie` is a session token, `headers` extra request headers,
        /// `body` a JSON text.
        pub async fn call(
            &self,
            method: &str,
            uri: &str,
            cookie: Option<&str>,
            headers: &[(&str, &str)],
            body: Option<&str>,
        ) -> Reply {
            self.call_bytes(
                method,
                uri,
                cookie,
                headers,
                body.map(|b| b.as_bytes().to_vec()),
            )
            .await
        }

        pub async fn call_bytes(
            &self,
            method: &str,
            uri: &str,
            cookie: Option<&str>,
            headers: &[(&str, &str)],
            body: Option<Vec<u8>>,
        ) -> Reply {
            let mut req = Request::builder().method(method).uri(uri);
            if let Some(token) = cookie {
                req = req.header(header::COOKIE, format!("cerbrix_session={token}"));
            }
            for (name, value) in headers {
                req = req.header(*name, *value);
            }
            let req = match body {
                Some(b) => req
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(b)),
                None => req.body(Body::empty()),
            };
            let resp = self.router.clone().oneshot(req.unwrap()).await.unwrap();
            let (parts, body) = resp.into_parts();
            Reply {
                status: parts.status,
                headers: parts.headers,
                body: body.collect().await.unwrap().to_bytes().to_vec(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use sqlx::PgPool;

    use super::testing::*;
    use super::*;

    #[test]
    fn routes_and_router_agree() {
        // Every registered handler is in ROUTES, and every ROUTES line has a handler. The
        // router also refuses to be built for a handler missing from ROUTES.
        let mut registered: Vec<(&str, &str)> =
            entries().iter().map(|e| (e.method, e.path)).collect();
        let mut listed: Vec<(&str, &str)> = ROUTES.iter().map(|(m, p, _)| (*m, *p)).collect();
        registered.sort_unstable();
        listed.sort_unstable();
        assert_eq!(registered, listed);
        let mut unique = listed.clone();
        unique.dedup();
        assert_eq!(unique, listed, "a route is listed twice");
        for (_, path, _) in ROUTES {
            assert!(path.starts_with("/api/v1/"), "{path}");
        }
    }

    #[test]
    #[should_panic(expected = "missing from ROUTES")]
    fn a_handler_without_a_routes_line_cannot_be_built() {
        async fn unlisted() {}
        // The lookup the router makes for every handler it registers.
        access_of(&Entry::new("GET", "/api/v1/not-listed", unlisted));
    }

    #[test]
    fn a_listed_handler_gets_its_access() {
        async fn listed() {}
        let entry = Entry::new("GET", "/api/v1/session", listed);
        assert_eq!(access_of(&entry), Access::Viewer);
    }

    /// The credentials the matrix tries, in the order of privilege.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Caller {
        Nobody,
        Role(Role),
    }

    fn allowed(access: Access, caller: Caller) -> bool {
        match (access, caller) {
            (Access::Public, _) => true,
            (Access::Display, _) => false,
            (_, Caller::Nobody) => false,
            (access, Caller::Role(role)) => access.role().is_some_and(|min| role >= min),
        }
    }

    /// Calls every route with no credentials, as viewer, operator and admin. A caller below
    /// the required level gets 401 (nobody) or 403 (a session); at or above it, neither.
    /// Later tasks add routes to `ROUTES` and, when needed, data to `Fixture`; they do not
    /// copy this test.
    #[sqlx::test(migrations = "./migrations")]
    async fn access_matrix(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        for (method, pattern, access) in ROUTES {
            let path = fx.fill(pattern);
            for caller in [
                Caller::Nobody,
                Caller::Role(Role::Viewer),
                Caller::Role(Role::Operator),
                Caller::Role(Role::Admin),
            ] {
                // A fresh session per request: some routes (logout) end the one they use.
                let token = match caller {
                    Caller::Nobody => None,
                    Caller::Role(role) => Some(fx.session(role).await),
                };
                let body = matches!(*method, "POST" | "PUT" | "PATCH").then_some("{}");
                let reply = fx.call(method, &path, token.as_deref(), &[], body).await;
                let what = format!("{method} {pattern} as {caller:?}: {}", reply.status);
                if allowed(*access, caller) {
                    assert_ne!(reply.status, StatusCode::UNAUTHORIZED, "{what}");
                    assert_ne!(reply.status, StatusCode::FORBIDDEN, "{what}");
                    // 404 and 405 would mean the table lists a route that is not served.
                    assert_ne!(reply.status, StatusCode::NOT_FOUND, "{what}");
                    assert_ne!(reply.status, StatusCode::METHOD_NOT_ALLOWED, "{what}");
                } else if caller == Caller::Nobody || *access == Access::Display {
                    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{what}");
                    assert_eq!(reply.error_code(), "unauthorized", "{what}");
                } else {
                    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{what}");
                    assert_eq!(reply.error_code(), "forbidden", "{what}");
                }
            }
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn an_invalid_session_gets_401_and_a_cleared_cookie(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let reply = fx
            .call("GET", "/api/v1/session", Some("cbx_se_unknown"), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        let cookie = reply.header("set-cookie").expect("the cookie is cleared");
        assert!(cookie.starts_with("cerbrix_session=;") && cookie.contains("Max-Age=0"));
        // No cookie at all: nothing to clear.
        let reply = fx.call("GET", "/api/v1/session", None, &[], None).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert!(reply.header("set-cookie").is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_disabled_user_and_an_expired_session_are_401(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let disabled = fx.session(Role::Viewer).await;
        sqlx::query("UPDATE users SET disabled = true WHERE id = $1")
            .bind(fx.viewer)
            .execute(&pool)
            .await
            .unwrap();
        let reply = fx
            .call("GET", "/api/v1/session", Some(&disabled), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);

        let expired = fx.session(Role::Admin).await;
        sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 second'")
            .execute(&pool)
            .await
            .unwrap();
        let reply = fx
            .call("GET", "/api/v1/session", Some(&expired), &[], None)
            .await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_role_change_applies_to_a_running_session(pool: PgPool) {
        // The role is read per request, not stored in the session.
        let fx = Fixture::new(pool.clone()).await;
        let token = fx.session(Role::Viewer).await;
        let reply = fx
            .call("GET", "/api/v1/session", Some(&token), &[], None)
            .await;
        assert_eq!(reply.json()["role"], "viewer");
        sqlx::query("UPDATE users SET role = 'operator' WHERE id = $1")
            .bind(fx.viewer)
            .execute(&pool)
            .await
            .unwrap();
        let reply = fx
            .call("GET", "/api/v1/session", Some(&token), &[], None)
            .await;
        assert_eq!(reply.json()["role"], "operator");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn responses_are_not_cacheable(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let token = fx.session(Role::Viewer).await;
        let ok = fx
            .call("GET", "/api/v1/session", Some(&token), &[], None)
            .await;
        let denied = fx.call("GET", "/api/v1/session", None, &[], None).await;
        let bad = fx
            .call(
                "POST",
                "/api/v1/session",
                None,
                &[("origin", "http://evil.example")],
                Some("{}"),
            )
            .await;
        for reply in [ok, denied, bad] {
            assert_eq!(reply.header("cache-control"), Some("no-store"));
        }
        // The agent routes are not touched.
        let health = fx.call("GET", "/healthz", None, &[], None).await;
        assert_eq!(health.header("cache-control"), None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn cross_origin_post_is_forbidden(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let login = format!(r#"{{"name":"viewer","password":"{PASSWORD}"}}"#);
        for headers in [
            vec![
                ("host", "cerbrix.example"),
                ("origin", "https://evil.example"),
            ],
            vec![
                ("host", "cerbrix.example"),
                ("origin", "https://cerbrix.example:8443"),
            ],
            vec![("host", "cerbrix.example"), ("origin", "null")],
            vec![("host", "cerbrix.example"), ("origin", "not a url")],
            vec![
                ("host", "cerbrix.example"),
                ("origin", "https://cerbrix.example/path"),
            ],
            vec![("origin", "https://cerbrix.example")], // no Host at all
            vec![("sec-fetch-site", "cross-site")],
            vec![("sec-fetch-site", "same-site")],
            vec![("sec-fetch-site", "none")],
            // Origin wins over Sec-Fetch-Site: a matching Sec-Fetch-Site cannot save it.
            vec![
                ("host", "cerbrix.example"),
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "same-origin"),
            ],
        ] {
            let reply = fx
                .call("POST", "/api/v1/session", None, &headers, Some(&login))
                .await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN, "{headers:?}");
            assert_eq!(reply.error_code(), "forbidden", "{headers:?}");
        }
        // A cross-origin GET is not subject to the rule (it changes nothing).
        let token = fx.session(Role::Viewer).await;
        let reply = fx
            .call(
                "GET",
                "/api/v1/session",
                Some(&token),
                &[("origin", "https://evil.example")],
                None,
            )
            .await;
        assert_eq!(reply.status, StatusCode::OK);
        // Neither session nor body is touched by a refused request: nobody was logged in.
        let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(&fx.pool)
            .await
            .unwrap();
        assert_eq!(sessions, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn same_origin_post_passes(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let login = format!(r#"{{"name":"viewer","password":"{PASSWORD}"}}"#);
        for headers in [
            vec![
                ("host", "cerbrix.example"),
                ("origin", "https://cerbrix.example"),
            ],
            vec![
                ("host", "Cerbrix.Example"),
                ("origin", "https://cerbrix.example"),
            ],
            vec![
                ("host", "cerbrix.example:8443"),
                ("origin", "https://cerbrix.example:8443"),
            ],
            vec![
                ("host", "cerbrix.example:443"),
                ("origin", "https://cerbrix.example"),
            ],
            vec![
                ("host", "cerbrix.example"),
                ("origin", "https://cerbrix.example:443"),
            ],
            vec![
                ("host", "cerbrix.example:80"),
                ("origin", "http://cerbrix.example"),
            ],
            vec![("host", "[::1]:8080"), ("origin", "http://[::1]:8080")],
            vec![("sec-fetch-site", "same-origin")],
            vec![], // not a browser: neither header
        ] {
            let reply = fx
                .call("POST", "/api/v1/session", None, &headers, Some(&login))
                .await;
            assert_eq!(reply.status, StatusCode::OK, "{headers:?}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn origin_with_other_scheme_but_same_host_passes(pool: PgPool) {
        // A TLS-terminating proxy forwards https traffic as plain HTTP: the Host header
        // names the same server whatever the scheme of the Origin.
        let fx = Fixture::new(pool).await;
        let login = format!(r#"{{"name":"viewer","password":"{PASSWORD}"}}"#);
        for (host, origin) in [
            ("cerbrix.example", "https://cerbrix.example"),
            ("cerbrix.example", "http://cerbrix.example"),
            ("cerbrix.example:8443", "https://cerbrix.example:8443"),
        ] {
            let reply = fx
                .call(
                    "POST",
                    "/api/v1/session",
                    None,
                    &[("host", host), ("origin", origin)],
                    Some(&login),
                )
                .await;
            assert_eq!(reply.status, StatusCode::OK, "{host} {origin}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn post_without_json_content_type_is_invalid(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let body = format!(r#"{{"name":"viewer","password":"{PASSWORD}"}}"#);
        for content_type in [
            None,
            Some("text/plain"),
            Some("application/x-www-form-urlencoded"),
        ] {
            let mut req = axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/session");
            if let Some(ct) = content_type {
                req = req.header("content-type", ct);
            }
            let resp = tower::ServiceExt::oneshot(
                fx.router.clone(),
                req.body(axum::body::Body::from(body.clone())).unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{content_type:?}");
        }
        // A charset parameter is fine.
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/session")
            .header("content-type", "application/json; charset=utf-8")
            .body(axum::body::Body::from(body))
            .unwrap();
        let resp = tower::ServiceExt::oneshot(fx.router.clone(), req)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn body_over_64_kib_is_rejected(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let json_of_len = |len: usize| {
            let pad = len - r#"{"name":"","password":"x"}"#.len();
            format!(r#"{{"name":"{}","password":"x"}}"#, "a".repeat(pad))
        };
        let exact = json_of_len(MAX_BODY_BYTES);
        assert_eq!(exact.len(), MAX_BODY_BYTES);
        let reply = fx
            .call("POST", "/api/v1/session", None, &[], Some(&exact))
            .await;
        assert_eq!(
            reply.status,
            StatusCode::UNAUTHORIZED,
            "at the limit: parsed, wrong login"
        );
        let over = json_of_len(MAX_BODY_BYTES + 1);
        let reply = fx
            .call("POST", "/api/v1/session", None, &[], Some(&over))
            .await;
        assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(reply.error_code(), "invalid");
        // The agent routes keep their own limits and authentication: a big body is not
        // refused by the UI limit, the missing credential is what stops it.
        let big = vec![b' '; MAX_BODY_BYTES + 1];
        let reply = fx
            .call_bytes("POST", "/api/agent/v1/values", None, &[], Some(big))
            .await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn origin_rules() {
        for (origin, host, ok) in [
            ("https://a.example", "a.example", true),
            ("https://A.EXAMPLE", "a.example", true),
            ("http://a.example:8080", "a.example:8080", true),
            ("http://a.example:8080", "a.example", false),
            ("https://a.example:443", "a.example", true),
            ("https://a.example", "a.example:443", true),
            ("https://a.example", "b.example", false),
            ("https://a.example", "a.example:444", false),
            ("https://a.example/", "a.example", true),
            ("https://a.example/x", "a.example", false),
            ("https://u@a.example", "a.example", false),
            ("https://a.example?x=1", "a.example", false),
            ("null", "a.example", false),
            ("", "a.example", false),
            ("ftp://a.example", "a.example", false),
            ("https://a.example", "", false),
            ("https://a.example", "a b", false),
        ] {
            assert_eq!(origin_matches_host(origin, host), ok, "{origin} vs {host}");
        }
    }

    #[tokio::test]
    async fn page_bounds() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt as _;

        let router = Router::new().route(
            "/p",
            get(|page: Page| async move { format!("{} {}", page.limit, page.offset) }),
        );
        let ask = |query: &'static str| {
            let router = router.clone();
            async move {
                let uri = format!("/p{query}");
                let resp = router
                    .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                let status = resp.status();
                let bytes = http_body_util::BodyExt::collect(resp.into_body())
                    .await
                    .unwrap()
                    .to_bytes();
                (status, String::from_utf8_lossy(&bytes).into_owned())
            }
        };
        assert_eq!(ask("").await, (StatusCode::OK, "50 0".to_owned()));
        assert_eq!(ask("?limit=1&offset=0").await.1, "1 0");
        assert_eq!(ask("?limit=500&offset=7").await.1, "500 7");
        for bad in [
            "?limit=0",
            "?limit=501",
            "?limit=-1",
            "?offset=-1",
            "?limit=x",
            "?offset=1.5",
        ] {
            let (status, body) = ask(bad).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
            assert!(body.contains("\"invalid\""), "{bad}: {body}");
        }
    }
}
