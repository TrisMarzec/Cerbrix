//! Agent-facing routes: health, enrolment, configuration and value ingest.

use axum::Json;
use axum::body::Body;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use cerbrix_common::{
    EnrollRequest, IngestResponse, MAX_BATCH_VALUES, MetricValue, PROTOCOL_VERSION, ValueBatch,
    default_agent_items,
};
use http_body_util::{BodyExt, LengthLimitError, Limited};
use serde::de::DeserializeOwned;
use time::OffsetDateTime;

use super::{ApiError, AppState};
use crate::enroll::{self, EnrollError};
use crate::gitops::item_config;
use crate::metrics::MetricsError;
use crate::state::Host;

/// Enrolment bodies are three short strings; anything larger is not a legitimate request.
const MAX_ENROLL_BODY_BYTES: usize = 16 * 1024;

pub(super) async fn healthz(State(app): State<AppState>) -> Response {
    match app.store.ping().await {
        Ok(()) => Json(serde_json::json!({ "status": "ok" })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "health check: state store unreachable");
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "state store unavailable",
            )
            .into_response()
        }
    }
}

pub(super) async fn enroll(State(app): State<AppState>, body: Body) -> Result<Response, ApiError> {
    let req: EnrollRequest = read_json(body, MAX_ENROLL_BODY_BYTES).await?;
    let result = enroll::enroll(&app.store, &req, OffsetDateTime::now_utc()).await;
    let err = match result {
        Ok(resp) => return Ok(Json(resp).into_response()),
        Err(e) => e,
    };
    let (status, message) = match &err {
        EnrollError::InvalidHostName => (StatusCode::BAD_REQUEST, "invalid host name"),
        EnrollError::InvalidKey => (StatusCode::UNAUTHORIZED, "invalid enrolment key"),
        EnrollError::UnknownHost => (StatusCode::NOT_FOUND, "unknown host"),
        EnrollError::AlreadyBound => (StatusCode::CONFLICT, "host is already enrolled"),
        EnrollError::State(e) => {
            tracing::error!(error = %e, "enrolment failed: state store error");
            return Err(ApiError::internal());
        }
    };
    Err(ApiError::new(status, err.code(), message))
}

/// The host a request authenticates as, taken from `Authorization: Bearer <credential>`.
/// Extracted from the request head, so it runs before the body is touched.
pub(super) struct AuthedHost(Host);

impl FromRequestParts<AppState> for AuthedHost {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        app: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let unauthorized = || {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing or invalid credential",
            )
        };
        let credential = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(bearer_token)
            .ok_or_else(unauthorized)?;
        match app.store.host_for_credential(credential).await {
            Ok(Some(host)) => Ok(AuthedHost(host)),
            Ok(None) => Err(unauthorized()),
            Err(e) => {
                tracing::error!(error = %e, "credential lookup failed");
                Err(ApiError::internal())
            }
        }
    }
}

fn bearer_token(header_value: &str) -> Option<&str> {
    let (scheme, token) = header_value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

pub(super) async fn values(
    State(app): State<AppState>,
    AuthedHost(host): AuthedHost,
    body: Body,
) -> Result<(StatusCode, Json<IngestResponse>), ApiError> {
    let batch: ValueBatch = read_json(body, app.config.ingest.max_body_bytes).await?;
    if batch.protocol_version != PROTOCOL_VERSION {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "unsupported_protocol",
            "unsupported protocol version",
        ));
    }
    if batch.values.len() > MAX_BATCH_VALUES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "batch_too_large",
            "too many values in one batch",
        ));
    }

    let now_ms = now_unix_ms();
    let skew = app.config.ingest.max_future_skew_secs;
    let total = batch.values.len();
    let accepted: Vec<MetricValue> = batch
        .values
        .into_iter()
        .filter(|v| v.validate().is_ok() && within_future_skew(v.ts_ms, now_ms, skew))
        .collect();
    let response = IngestResponse {
        accepted: accepted.len(),
        rejected: total - accepted.len(),
    };

    if !accepted.is_empty() {
        match app.metrics.write(&host.name, &accepted).await {
            Ok(()) => {}
            Err(MetricsError::Unavailable(e)) => {
                tracing::error!(host = %host.name, error = %e, "metrics store unavailable");
                return Err(ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "metrics_unavailable",
                    "metrics store unavailable, retry later",
                ));
            }
            Err(MetricsError::Rejected(e)) => {
                tracing::error!(host = %host.name, error = %e, "metrics store rejected a batch");
                return Err(ApiError::internal());
            }
        }
    }
    Ok((StatusCode::ACCEPTED, Json(response)))
}

/// The item configuration of the authenticated host. The version is the ETag; a matching
/// `If-None-Match` is answered with 304 and no body. A host without stored configuration
/// (created manually, not from Git) gets the default items.
pub(super) async fn config(
    State(app): State<AppState>,
    AuthedHost(host): AuthedHost,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let config = match app.store.host_config(host.id).await {
        Ok(Some(effective)) => effective.agent_config(),
        Ok(None) => item_config(default_agent_items()),
        Err(e) => {
            tracing::error!(host = %host.name, error = %e, "loading host configuration failed");
            return Err(ApiError::internal());
        }
    };
    // The version is lower-case hex, so the quoted form is always a valid header value.
    let etag = format!("\"{}\"", config.version);
    let matches = headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| if_none_match_hits(v, &config.version));
    let status = if matches {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let response_headers = [
        (header::ETAG, etag),
        (header::CACHE_CONTROL, "no-cache".to_owned()),
    ];
    if matches {
        Ok((status, response_headers).into_response())
    } else {
        Ok((status, response_headers, Json(config)).into_response())
    }
}

/// True when an `If-None-Match` value names `version`: `*`, or a comma-separated list with
/// the strong or weak form of its ETag (weak comparison, as RFC 9110 prescribes here).
fn if_none_match_hits(value: &str, version: &str) -> bool {
    value.split(',').map(str::trim).any(|candidate| {
        candidate == "*"
            || candidate
                .strip_prefix("W/")
                .unwrap_or(candidate)
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                == Some(version)
    })
}

/// Reads at most `limit` bytes of body and parses them as JSON.
async fn read_json<T: DeserializeOwned>(body: Body, limit: usize) -> Result<T, ApiError> {
    let bytes = match Limited::new(body, limit).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) if e.downcast_ref::<LengthLimitError>().is_some() => {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "request body too large",
            ));
        }
        Err(_) => return Err(malformed()),
    };
    serde_json::from_slice(&bytes).map_err(|_| malformed())
}

fn malformed() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "malformed",
        "request body is not valid",
    )
}

/// The system clock in Unix milliseconds; the only place the ingest path reads the clock.
fn now_unix_ms() -> i64 {
    i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
}

/// True unless `ts_ms` is more than `max_skew_secs` ahead of `now_ms`.
fn within_future_skew(ts_ms: i64, now_ms: i64, max_skew_secs: u64) -> bool {
    i128::from(ts_ms) <= i128::from(now_ms) + i128::from(max_skew_secs) * 1000
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use cerbrix_common::{
        ErrorBody, IngestResponse, MAX_BATCH_VALUES, MetricValue, PROTOCOL_VERSION, ValueBatch,
    };
    use http_body_util::BodyExt;
    use sqlx::PgPool;
    use time::{Duration, OffsetDateTime};
    use tower::ServiceExt;

    use super::*;
    use crate::api::{AppState, router};
    use crate::config::{Config, DatabaseConfig, IngestConfig, MetricsConfig, ServerConfig};
    use crate::metrics::MemoryMetrics;
    use crate::state::StateStore;

    fn config(max_body_bytes: usize) -> Arc<Config> {
        Arc::new(Config {
            server: ServerConfig {
                listen: SocketAddr::from(([127, 0, 0, 1], 0)),
                tls_cert: None,
                tls_key: None,
                insecure_http: true,
            },
            database: DatabaseConfig {
                url: "postgres://localhost/unused".into(),
            },
            metrics: MetricsConfig {
                url: "http://localhost:8428".into(),
            },
            ingest: IngestConfig {
                max_body_bytes,
                ..IngestConfig::default()
            },
            gitops: Default::default(),
        })
    }

    fn app_with(pool: PgPool, max_body_bytes: usize) -> (Router, Arc<MemoryMetrics>, StateStore) {
        let store = StateStore::from_pool(pool);
        let metrics = Arc::new(MemoryMetrics::new());
        let router = router(AppState {
            store: store.clone(),
            metrics: metrics.clone(),
            config: config(max_body_bytes),
        });
        (router, metrics, store)
    }

    fn app(pool: PgPool) -> (Router, Arc<MemoryMetrics>, StateStore) {
        app_with(pool, 2 * 1024 * 1024)
    }

    fn now_ms() -> i64 {
        now_unix_ms()
    }

    fn value(key: &str, ts_ms: i64) -> MetricValue {
        MetricValue {
            key: key.into(),
            params: BTreeMap::new(),
            ts_ms,
            value: 1.5,
        }
    }

    fn batch(values: Vec<MetricValue>) -> ValueBatch {
        ValueBatch {
            protocol_version: PROTOCOL_VERSION,
            values,
        }
    }

    async fn send(
        router: &Router,
        method: &str,
        uri: &str,
        credential: Option<&str>,
        body: Vec<u8>,
    ) -> (StatusCode, Vec<u8>) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = credential {
            req = req.header(header::AUTHORIZATION, format!("Bearer {c}"));
        }
        let resp = router
            .clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, bytes.to_vec())
    }

    async fn post_values(
        router: &Router,
        credential: Option<&str>,
        b: &ValueBatch,
    ) -> (StatusCode, Vec<u8>) {
        send(
            router,
            "POST",
            "/api/agent/v1/values",
            credential,
            serde_json::to_vec(b).unwrap(),
        )
        .await
    }

    async fn post_enroll(router: &Router, host: &str, key: &str) -> (StatusCode, Vec<u8>) {
        let body = serde_json::json!({
            "host": host, "enrollment_key": key, "agent_version": "0.1.0"
        });
        send(
            router,
            "POST",
            "/api/agent/v1/enroll",
            None,
            serde_json::to_vec(&body).unwrap(),
        )
        .await
    }

    fn error_code(body: &[u8]) -> String {
        serde_json::from_slice::<ErrorBody>(body).unwrap().error
    }

    async fn enrolled(router: &Router, store: &StateStore, host: &str) -> String {
        store.create_host(host).await.unwrap();
        let (_, key) = store
            .create_enrollment_key("t", OffsetDateTime::now_utc() + Duration::hours(1))
            .await
            .unwrap();
        let (status, body) = post_enroll(router, host, &key).await;
        assert_eq!(status, StatusCode::OK);
        serde_json::from_slice::<cerbrix_common::EnrollResponse>(&body)
            .unwrap()
            .credential
    }

    #[test]
    fn future_skew_rule() {
        let now = 1_000_000;
        assert!(within_future_skew(now + 300_000, now, 300));
        assert!(!within_future_skew(now + 300_001, now, 300));
        assert!(within_future_skew(0, now, 300));
        assert!(within_future_skew(i64::MIN, now, 300));
        assert!(!within_future_skew(i64::MAX, 0, 300));
    }

    #[test]
    fn bearer_parsing() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer abc"), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("abc"), None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn healthz_ok(pool: PgPool) {
        let (router, _, _) = app(pool);
        let (status, body) = send(&router, "GET", "/healthz", None, vec![]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"status": "ok"})
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn enroll_then_send_values_stores_them_under_the_host(pool: PgPool) {
        let (router, metrics, store) = app(pool);
        let cred = enrolled(&router, &store, "web-01").await;
        let b = batch(vec![
            value("cpu.usage", now_ms()),
            value("mem.used", now_ms()),
        ]);
        let (status, body) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let resp: IngestResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            resp,
            IngestResponse {
                accepted: 2,
                rejected: 0
            }
        );
        let written = metrics.written();
        assert_eq!(written.len(), 2);
        assert!(written.iter().all(|(h, _)| h == "web-01"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn values_without_credential_are_unauthorized(pool: PgPool) {
        let (router, metrics, _) = app(pool);
        let b = batch(vec![value("cpu.usage", now_ms())]);
        let (status, body) = post_values(&router, None, &b).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error_code(&body), "unauthorized");
        let (status, _) = post_values(&router, Some("cbx_ac_nope"), &b).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(metrics.written().is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unauthorized_wins_over_bad_body(pool: PgPool) {
        let (router, _, _) = app_with(pool, 16);
        let (status, _) = send(
            &router,
            "POST",
            "/api/agent/v1/values",
            None,
            vec![b'x'; 1000],
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn values_with_credential_of_deactivated_host_are_unauthorized(pool: PgPool) {
        let (router, metrics, store) = app(pool);
        let cred = enrolled(&router, &store, "web-01").await;
        let host = store.host_by_name("web-01").await.unwrap().unwrap();
        store.set_host_active(host.id, false).await.unwrap();
        let b = batch(vec![value("cpu.usage", now_ms())]);
        let (status, body) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error_code(&body), "unauthorized");
        assert!(metrics.written().is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn wrong_protocol_version_is_rejected(pool: PgPool) {
        let (router, metrics, store) = app(pool);
        let cred = enrolled(&router, &store, "web-01").await;
        let mut b = batch(vec![value("cpu.usage", now_ms())]);
        b.protocol_version = PROTOCOL_VERSION + 1;
        let (status, body) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error_code(&body), "unsupported_protocol");
        assert!(metrics.written().is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn malformed_and_oversized_bodies_are_rejected(pool: PgPool) {
        let (router, metrics, store) = app_with(pool, 256);
        let cred = enrolled(&router, &store, "web-01").await;
        let uri = "/api/agent/v1/values";
        let (status, body) = send(&router, "POST", uri, Some(&cred), b"{nope".to_vec()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error_code(&body), "malformed");
        let (status, body) = send(&router, "POST", uri, Some(&cred), vec![b' '; 257]).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error_code(&body), "body_too_large");
        assert!(metrics.written().is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn oversized_batch_is_rejected(pool: PgPool) {
        let (router, metrics, store) = app(pool);
        let cred = enrolled(&router, &store, "web-01").await;
        let b = batch(vec![value("cpu.usage", now_ms()); MAX_BATCH_VALUES + 1]);
        let (status, body) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error_code(&body), "batch_too_large");
        assert!(metrics.written().is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn future_and_invalid_values_are_dropped_not_fatal(pool: PgPool) {
        let (router, metrics, store) = app(pool);
        let cred = enrolled(&router, &store, "web-01").await;
        let b = batch(vec![
            value("cpu.usage", now_ms()),
            value("cpu.usage", now_ms() + 3_600_000),
            value("BAD", now_ms()),
        ]);
        let (status, body) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let resp: IngestResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            resp,
            IngestResponse {
                accepted: 1,
                rejected: 2
            }
        );
        assert_eq!(metrics.written().len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn fully_rejected_batch_is_accepted_with_zero_and_writes_nothing(pool: PgPool) {
        let (router, metrics, store) = app(pool);
        let cred = enrolled(&router, &store, "web-01").await;
        // The store is down, but nothing valid is sent, so it is never called.
        metrics.set_failing(true);
        let b = batch(vec![value("BAD", now_ms())]);
        let (status, body) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let resp: IngestResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            resp,
            IngestResponse {
                accepted: 0,
                rejected: 1
            }
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn metrics_outage_returns_503_and_writes_nothing(pool: PgPool) {
        let (router, metrics, store) = app(pool);
        let cred = enrolled(&router, &store, "web-01").await;
        metrics.set_failing(true);
        let b = batch(vec![value("cpu.usage", now_ms())]);
        let (status, body) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error_code(&body), "metrics_unavailable");
        assert!(metrics.written().is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn enroll_error_codes_map_to_statuses(pool: PgPool) {
        let (router, _, store) = app(pool);
        store.create_host("web-01").await.unwrap();
        let (_, key) = store
            .create_enrollment_key("t", OffsetDateTime::now_utc() + Duration::hours(1))
            .await
            .unwrap();

        let (s, b) = post_enroll(&router, "bad name", &key).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(error_code(&b), "invalid_host_name");
        let (s, b) = post_enroll(&router, "web-01", "cbx_ek_wrong").await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(error_code(&b), "invalid_key");
        let (s, b) = post_enroll(&router, "ghost", &key).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(error_code(&b), "unknown_host");
        let (s, _) = post_enroll(&router, "web-01", &key).await;
        assert_eq!(s, StatusCode::OK);
        let (s, b) = post_enroll(&router, "web-01", &key).await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(error_code(&b), "already_bound");

        let (s, b) = send(&router, "POST", "/api/agent/v1/enroll", None, b"{".to_vec()).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(error_code(&b), "malformed");
    }

    const CONFIG_URI: &str = "/api/agent/v1/config";

    async fn get_config(
        router: &Router,
        credential: Option<&str>,
        if_none_match: Option<&str>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut req = Request::builder().method("GET").uri(CONFIG_URI);
        if let Some(c) = credential {
            req = req.header(header::AUTHORIZATION, format!("Bearer {c}"));
        }
        if let Some(v) = if_none_match {
            req = req.header(header::IF_NONE_MATCH, v);
        }
        let resp = router
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, bytes.to_vec())
    }

    fn example_compiled(commit: &str) -> crate::gitops::Compiled {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        crate::gitops::compile(&crate::gitops::read_tree(&dir, commit).unwrap()).unwrap()
    }

    fn apply_time() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    /// Applies the example repository and enrols `web-01`; returns its credential.
    async fn enrol_web_01(store: &StateStore) -> String {
        store
            .apply_config("commit-a", &example_compiled("commit-a"), apply_time(), 50)
            .await
            .unwrap();
        let host = store.host_by_name("web-01").await.unwrap().unwrap();
        store.bind_credential(host.id).await.unwrap()
    }

    fn etag(headers: &axum::http::HeaderMap) -> String {
        headers
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn config_requires_credential(pool: PgPool) {
        let (router, _, _) = app(pool);
        let (status, _, body) = get_config(&router, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error_code(&body), "unauthorized");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn config_returns_effective_items(pool: PgPool) {
        let (router, _, store) = app(pool);
        let cred = enrol_web_01(&store).await;
        let (status, headers, body) = get_config(&router, Some(&cred), None).await;
        assert_eq!(status, StatusCode::OK);
        let config: cerbrix_common::ItemConfig = serde_json::from_slice(&body).unwrap();
        let cpu = config.items.iter().find(|i| i.key == "cpu.usage").unwrap();
        assert_eq!(cpu.interval_secs, 10);
        assert_eq!(etag(&headers), format!("\"{}\"", config.version));
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-cache");
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["items", "version"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn config_not_modified(pool: PgPool) {
        let (router, _, store) = app(pool);
        let cred = enrol_web_01(&store).await;
        let (_, headers, _) = get_config(&router, Some(&cred), None).await;
        let tag = etag(&headers);
        let version = tag.trim_matches('"').to_owned();

        for matching in [
            tag.clone(),
            format!("W/{tag}"),
            format!("\"other\", {tag}"),
            "*".to_owned(),
        ] {
            let (status, h304, body) = get_config(&router, Some(&cred), Some(&matching)).await;
            assert_eq!(status, StatusCode::NOT_MODIFIED, "{matching}");
            assert!(body.is_empty());
            assert_eq!(etag(&h304), tag);
            assert_eq!(h304.get(header::CACHE_CONTROL).unwrap(), "no-cache");
        }
        for other in ["\"other\"", &version, "W/", "", "\"a\" \"b\""] {
            let (status, _, body) = get_config(&router, Some(&cred), Some(other)).await;
            assert_eq!(status, StatusCode::OK, "{other}");
            assert!(!body.is_empty());
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn config_version_changes_after_sync(pool: PgPool) {
        let (router, _, store) = app(pool);
        let cred = enrol_web_01(&store).await;
        let (_, before, _) = get_config(&router, Some(&cred), None).await;

        let mut changed = example_compiled("commit-b");
        let web = changed
            .hosts
            .iter_mut()
            .find(|h| h.name == "web-01")
            .unwrap();
        let cpu = web
            .effective
            .items
            .iter_mut()
            .find(|i| i.key == "cpu.usage")
            .unwrap();
        cpu.interval_secs = 30;
        store
            .apply_config(
                "commit-b",
                &changed,
                apply_time() + Duration::minutes(1),
                50,
            )
            .await
            .unwrap();

        let (status, after, body) = get_config(&router, Some(&cred), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(etag(&before), etag(&after));
        let config: cerbrix_common::ItemConfig = serde_json::from_slice(&body).unwrap();
        let cpu = config.items.iter().find(|i| i.key == "cpu.usage").unwrap();
        assert_eq!(cpu.interval_secs, 30);
        let (status, _, _) = get_config(&router, Some(&cred), Some(&etag(&before))).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn manual_host_gets_defaults(pool: PgPool) {
        let (router, _, store) = app(pool);
        let cred = enrolled(&router, &store, "manual-01").await;
        let (status, headers, body) = get_config(&router, Some(&cred), None).await;
        assert_eq!(status, StatusCode::OK);
        let config: cerbrix_common::ItemConfig = serde_json::from_slice(&body).unwrap();
        assert_eq!(config.items.len(), 17);
        assert_eq!(
            config,
            crate::gitops::item_config(cerbrix_common::default_agent_items())
        );
        assert_eq!(etag(&headers), format!("\"{}\"", config.version));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn removed_host_is_unauthorized(pool: PgPool) {
        let (router, _, store) = app(pool);
        let cred = enrol_web_01(&store).await;
        let mut without = example_compiled("commit-b");
        without.hosts.retain(|h| h.name != "web-01");
        store
            .apply_config(
                "commit-b",
                &without,
                apply_time() + Duration::minutes(1),
                100,
            )
            .await
            .unwrap();
        let (status, _, body) = get_config(&router, Some(&cred), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error_code(&body), "unauthorized");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn second_router_instance_serves_the_same_credential(pool: PgPool) {
        let (first, _, store) = app(pool.clone());
        let (second, metrics, _) = app(pool);
        let cred = enrolled(&first, &store, "web-01").await;
        let b = batch(vec![value("cpu.usage", now_ms())]);
        let (status, _) = post_values(&second, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(metrics.written().len(), 1);
    }
}
