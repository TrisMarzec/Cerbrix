//! Agent-facing routes: health, enrolment, configuration and value ingest.

use std::collections::BTreeMap;
use std::time::Instant;

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

use cerbrix_common::{Selector, TriggerExpr};

use super::{ApiError, AppState};
use crate::alerting::eval::evaluate;
use crate::alerting::{ActiveTrigger, Params, active_triggers, instance_key};
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
    headers: HeaderMap,
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
        let user_agent = headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok());
        let version = user_agent.and_then(agent_version_from_user_agent);
        evaluate_arrival(&app, &host, &accepted, now_ms, version).await?;
    }
    Ok((StatusCode::ACCEPTED, Json(response)))
}

/// The version in a `User-Agent` of the form `cerbrix-agent/<version>`; a header of any other
/// form, or with a version that is not valid (see `crate::enroll::valid_agent_version`), names
/// no version.
fn agent_version_from_user_agent(user_agent: &str) -> Option<&str> {
    let version = user_agent.strip_prefix("cerbrix-agent/")?;
    crate::enroll::valid_agent_version(version).then_some(version)
}

/// Evaluates the triggers of `host` against the stored values and records the arrival.
/// The values are already in the metrics store; an error here makes the agent resend them,
/// which is harmless: evaluation ignores values it has already seen.
async fn evaluate_arrival(
    app: &AppState,
    host: &Host,
    accepted: &[MetricValue],
    now_ms: i64,
    agent_version: Option<&str>,
) -> Result<(), ApiError> {
    let max_age_ms = i64::try_from(app.config.alerting.max_value_age_secs.saturating_mul(1000))
        .unwrap_or(i64::MAX);
    // Filled by `decide`, which is the only place the host's triggers are at hand.
    let mut series = Vec::new();
    let mut too_old = 0;
    let evaluated = app
        .store
        .evaluate_host(host.id, app.queue_mail, |ctx| {
            let triggers = ctx.config.as_ref().map(active_triggers).unwrap_or_default();
            series = nodata_series(&triggers, accepted);
            let evaluation = evaluate(&triggers, &ctx.states, accepted, now_ms, max_age_ms);
            too_old = evaluation.too_old;
            evaluation
        })
        .await;
    let arrived = OffsetDateTime::now_utc();
    let recorded = match evaluated {
        Ok(_) => {
            app.store
                .touch_seen_versioned(host.id, arrived, &series, agent_version)
                .await
        }
        Err(e) => Err(e),
    };
    if let Err(e) = recorded {
        tracing::error!(host = %host.name, error = %e, "recording alerting state failed");
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "alerting_unavailable",
            "alerting state unavailable, retry later",
        ));
    }
    if too_old > 0 && too_old == accepted.len() && app.warnings.should_warn(host.id, Instant::now())
    {
        tracing::warn!(
            host = %host.name,
            values = too_old,
            tolerance_secs = app.config.alerting.max_value_age_secs,
            "stored values older than the tolerance without evaluating them"
        );
    }
    Ok(())
}

/// The series (item key and parameters, once each) among `values` that a no-data trigger
/// with a selector watches.
fn nodata_series(triggers: &[ActiveTrigger], values: &[MetricValue]) -> Vec<(String, Params)> {
    let selectors: Vec<&Selector> = triggers
        .iter()
        .filter_map(|t| match &t.expr {
            TriggerExpr::NoData {
                selector: Some(s), ..
            } => Some(s),
            _ => None,
        })
        .collect();
    let mut found: BTreeMap<(&str, String), &MetricValue> = BTreeMap::new();
    for v in values {
        let watched = selectors.iter().any(|s| {
            s.key == v.key
                && s.params
                    .iter()
                    .all(|(k, want)| v.params.get(k) == Some(want))
        });
        if watched {
            found
                .entry((v.key.as_str(), instance_key(&v.params)))
                .or_insert(v);
        }
    }
    found
        .into_values()
        .map(|v| (v.key.clone(), v.params.clone()))
        .collect()
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
            alerting: Default::default(),
            smtp: Default::default(),
            auth: Default::default(),
        })
    }

    fn app_with(pool: PgPool, max_body_bytes: usize) -> (Router, Arc<MemoryMetrics>, StateStore) {
        let store = StateStore::from_pool(pool);
        let metrics = Arc::new(MemoryMetrics::new());
        let router = router(AppState {
            store: store.clone(),
            metrics: metrics.clone(),
            config: config(max_body_bytes),
            warnings: Arc::default(),
            queue_mail: false,
            passwords: Default::default(),
            logins: Arc::default(),
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

    #[test]
    fn agent_version_is_read_from_the_user_agent() {
        assert_eq!(
            agent_version_from_user_agent("cerbrix-agent/1.2.3"),
            Some("1.2.3")
        );
        assert_eq!(
            agent_version_from_user_agent("cerbrix-agent/0.1.0-rc.1+b5"),
            Some("0.1.0-rc.1+b5")
        );
        let longest = format!("cerbrix-agent/{}", "1".repeat(64));
        assert_eq!(
            agent_version_from_user_agent(&longest),
            Some(&longest[14..])
        );
        for bad in [
            "",
            "cerbrix-agent/",
            "curl/8.0",
            "Cerbrix-Agent/1.2.3",
            "cerbrix-agent/1.2.3 extra",
            "cerbrix-agent/1 2",
            "cerbrix-agent/1.2.3
",
            "cerbrix-agent/ä",
            "prefix cerbrix-agent/1.2.3",
        ] {
            assert_eq!(agent_version_from_user_agent(bad), None, "{bad:?}");
        }
        let too_long = format!("cerbrix-agent/{}", "1".repeat(65));
        assert_eq!(agent_version_from_user_agent(&too_long), None);
    }

    async fn post_values_as(
        router: &Router,
        credential: &str,
        user_agent: Option<&str>,
    ) -> StatusCode {
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/agent/v1/values")
            .header(header::AUTHORIZATION, format!("Bearer {credential}"));
        if let Some(ua) = user_agent {
            req = req.header(header::USER_AGENT, ua);
        }
        let body = serde_json::to_vec(&batch(vec![value("cpu.usage", now_ms())])).unwrap();
        router
            .clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap()
            .status()
    }

    async fn stored_version(pool: &PgPool) -> Option<String> {
        sqlx::query_scalar("SELECT agent_version FROM hosts WHERE name = 'web-01'")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn values_take_the_version_from_the_user_agent(pool: PgPool) {
        let (router, _, store) = app(pool.clone());
        let cred = enrolled(&router, &store, "web-01").await;
        // The version of the enrolment request is stored at enrolment.
        assert_eq!(stored_version(&pool).await.as_deref(), Some("0.1.0"));
        let status = post_values_as(&router, &cred, Some("cerbrix-agent/1.2.3")).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(stored_version(&pool).await.as_deref(), Some("1.2.3"));
        // A header that names no version is accepted and leaves the stored one alone.
        let too_long = format!("cerbrix-agent/{}", "9".repeat(65));
        for ua in [
            None,
            Some("curl/8.0"),
            Some("cerbrix-agent/"),
            Some(&*too_long),
        ] {
            assert_eq!(
                post_values_as(&router, &cred, ua).await,
                StatusCode::ACCEPTED
            );
            assert_eq!(stored_version(&pool).await.as_deref(), Some("1.2.3"));
        }
        let status = post_values_as(&router, &cred, Some("cerbrix-agent/1.3.0")).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(stored_version(&pool).await.as_deref(), Some("1.3.0"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_missing_or_malformed_user_agent_leaves_the_enrolment_version(pool: PgPool) {
        let (router, _, store) = app(pool.clone());
        let cred = enrolled(&router, &store, "web-01").await;
        for ua in [None, Some("cerbrix-agent/1.2 3"), Some("curl/8.0")] {
            assert_eq!(
                post_values_as(&router, &cred, ua).await,
                StatusCode::ACCEPTED
            );
        }
        assert_eq!(stored_version(&pool).await.as_deref(), Some("0.1.0"));
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
            .apply_config(
                "commit-a",
                &example_compiled("commit-a"),
                apply_time(),
                50,
                false,
            )
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
                false,
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
                false,
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

    // ---- alerting on ingest ----

    /// Applies the example repository with `web-01` carrying `triggers` (the example
    /// notification rule mails ops@example.com for `high` and above in `production`)
    /// and binds its credential.
    async fn enrol_with_triggers(store: &StateStore, triggers: serde_json::Value) -> String {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        let mut snapshot = crate::gitops::read_tree(&dir, "commit-t").unwrap();
        let host_file = serde_json::json!({
            "host": "web-01",
            "groups": ["production", "web"],
            "items": [
                { "key": "cpu.usage", "interval": "10s" },
                { "key": "disk.used_percent", "interval": "1m" },
            ],
            "triggers": triggers,
        });
        snapshot.files.insert(
            "hosts/web-01.json".to_owned(),
            serde_json::to_vec(&host_file).unwrap(),
        );
        let compiled = crate::gitops::compile(&snapshot).unwrap();
        store
            .apply_config("commit-t", &compiled, apply_time(), 50, false)
            .await
            .unwrap();
        let host = store.host_by_name("web-01").await.unwrap().unwrap();
        store.bind_credential(host.id).await.unwrap()
    }

    fn cpu_trigger() -> serde_json::Value {
        serde_json::json!([
            { "name": "CPU high", "expression": "cpu.usage > 90", "severity": "critical" }
        ])
    }

    fn cpu(value: f64, ts_ms: i64) -> MetricValue {
        MetricValue {
            value,
            ..self::value("cpu.usage", ts_ms)
        }
    }

    async fn open_problems(store: &StateStore) -> Vec<crate::state::Problem> {
        store
            .problems(crate::state::ProblemFilter::Open, 100)
            .await
            .unwrap()
    }

    async fn last_seen(pool: &PgPool, host: &str) -> Option<OffsetDateTime> {
        sqlx::query_scalar("SELECT last_seen_at FROM hosts WHERE name = $1")
            .bind(host)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn app_with_mail(pool: PgPool, queue_mail: bool) -> (Router, StateStore) {
        let store = StateStore::from_pool(pool);
        let router = router(AppState {
            store: store.clone(),
            metrics: Arc::new(MemoryMetrics::new()),
            config: config(2 * 1024 * 1024),
            warnings: Arc::default(),
            queue_mail,
            passwords: Default::default(),
            logins: Arc::default(),
        });
        (router, store)
    }

    async fn outbox_recipients(pool: &PgPool) -> Vec<String> {
        sqlx::query_scalar("SELECT recipient FROM notification_outbox ORDER BY recipient")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn triggering_value_queues_mail_when_smtp_is_configured(pool: PgPool) {
        let (router, store) = app_with_mail(pool.clone(), true);
        let cred = enrol_with_triggers(&store, cpu_trigger()).await;
        let b = batch(vec![cpu(95.0, now_ms())]);
        let (status, _) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(open_problems(&store).await.len(), 1);
        assert_eq!(outbox_recipients(&pool).await, ["ops@example.com"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn triggering_value_queues_nothing_without_smtp(pool: PgPool) {
        let (router, store) = app_with_mail(pool.clone(), false);
        let cred = enrol_with_triggers(&store, cpu_trigger()).await;
        let b = batch(vec![cpu(95.0, now_ms())]);
        let (status, _) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(open_problems(&store).await.len(), 1);
        assert!(outbox_recipients(&pool).await.is_empty());
    }
    #[sqlx::test(migrations = "./migrations")]
    async fn value_above_threshold_opens_a_problem(pool: PgPool) {
        let (router, _, store) = app(pool);
        let cred = enrol_with_triggers(&store, cpu_trigger()).await;
        let b = batch(vec![cpu(95.0, now_ms())]);
        let (status, _) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let open = open_problems(&store).await;
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].host, "web-01");
        assert_eq!(open[0].trigger, "CPU high");
        assert_eq!(open[0].severity, cerbrix_common::config::Severity::Critical);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn value_below_threshold_resolves_it(pool: PgPool) {
        let (router, _, store) = app(pool);
        let cred = enrol_with_triggers(&store, cpu_trigger()).await;
        let now = now_ms();
        post_values(&router, Some(&cred), &batch(vec![cpu(95.0, now - 2000)])).await;
        assert_eq!(open_problems(&store).await.len(), 1);
        let b = batch(vec![cpu(40.0, now - 1000)]);
        let (status, _) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(open_problems(&store).await.is_empty());
        let all = store
            .problems(crate::state::ProblemFilter::All, 10)
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].resolved_at.is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn resending_the_batch_opens_nothing_new(pool: PgPool) {
        let (router, _, store) = app(pool);
        let cred = enrol_with_triggers(&store, cpu_trigger()).await;
        let b = batch(vec![cpu(95.0, now_ms())]);
        for _ in 0..2 {
            let (status, _) = post_values(&router, Some(&cred), &b).await;
            assert_eq!(status, StatusCode::ACCEPTED);
        }
        let all = store
            .problems(crate::state::ProblemFilter::All, 10)
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn old_values_are_stored_but_not_evaluated(pool: PgPool) {
        let (router, metrics, store) = app(pool.clone());
        let cred = enrol_with_triggers(&store, cpu_trigger()).await;
        let old = now_ms() - 10 * 60 * 1000;
        let (status, _) = post_values(&router, Some(&cred), &batch(vec![cpu(95.0, old)])).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(metrics.written().len(), 1);
        assert!(open_problems(&store).await.is_empty());
        // The value did arrive.
        assert!(last_seen(&pool, "web-01").await.is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn store_failure_answers_503_and_keeps_semantics(pool: PgPool) {
        let (router, metrics, store) = app(pool.clone());
        let cred = enrol_with_triggers(&store, cpu_trigger()).await;
        sqlx::query("ALTER TABLE trigger_states RENAME TO trigger_states_gone")
            .execute(&pool)
            .await
            .unwrap();
        let b = batch(vec![cpu(95.0, now_ms())]);
        let (status, body) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error_code(&body), "alerting_unavailable");
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("alerting state unavailable, retry later"));
        assert!(!text.contains("trigger_states"));
        // The values were written first, so a resend writes the same samples again.
        assert_eq!(metrics.written().len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_without_configuration_is_fine(pool: PgPool) {
        let (router, metrics, store) = app(pool.clone());
        let cred = enrolled(&router, &store, "manual-01").await;
        let b = batch(vec![cpu(95.0, now_ms())]);
        let (status, _) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(metrics.written().len(), 1);
        assert!(open_problems(&store).await.is_empty());
        assert!(last_seen(&pool, "manual-01").await.is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn last_seen_is_updated(pool: PgPool) {
        let (router, _, store) = app(pool.clone());
        let cred = enrol_with_triggers(&store, serde_json::json!([])).await;
        assert_eq!(last_seen(&pool, "web-01").await, None);

        // A batch without any accepted value does not count as arrival.
        post_values(&router, Some(&cred), &batch(vec![value("BAD", now_ms())])).await;
        assert_eq!(last_seen(&pool, "web-01").await, None);

        let before = OffsetDateTime::now_utc();
        let b = batch(vec![cpu(1.0, now_ms())]);
        let (status, _) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let seen = last_seen(&pool, "web-01").await.unwrap();
        assert!(seen >= before - Duration::seconds(1));
        assert!(seen <= OffsetDateTime::now_utc() + Duration::seconds(1));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn series_seen_only_for_nodata_selectors(pool: PgPool) {
        let (router, _, store) = app(pool.clone());
        let triggers = serde_json::json!([
            { "name": "Root silent", "expression": "disk.used_percent{mount=/} no data for 5m",
              "severity": "warning" },
            { "name": "Host silent", "expression": "no data for 3m", "severity": "critical" },
            { "name": "CPU high", "expression": "cpu.usage > 90", "severity": "critical" },
        ]);
        let cred = enrol_with_triggers(&store, triggers).await;
        let disk = |mount: &str| MetricValue {
            params: BTreeMap::from([("mount".to_owned(), mount.to_owned())]),
            ..value("disk.used_percent", now_ms())
        };
        let b = batch(vec![
            disk("/"),
            disk("/"),
            disk("/data"),
            cpu(1.0, now_ms()),
            value("mem.used", now_ms()),
        ]);
        let (status, _) = post_values(&router, Some(&cred), &b).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let rows: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
            "SELECT item_key, instance, params FROM series_seen ORDER BY item_key, instance",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            [(
                "disk.used_percent".to_owned(),
                "mount=/".to_owned(),
                serde_json::json!({"mount": "/"})
            )]
        );
    }

    #[test]
    fn warns_once_per_hour_per_host() {
        let limiter = crate::api::WarnLimiter::default();
        let (a, b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let t0 = std::time::Instant::now();
        let after = |secs| t0 + std::time::Duration::from_secs(secs);
        assert!(limiter.should_warn(a, t0));
        assert!(!limiter.should_warn(a, after(10)));
        assert!(limiter.should_warn(b, after(10)));
        assert!(!limiter.should_warn(a, after(3599)));
        assert!(limiter.should_warn(a, after(3600)));
        assert!(!limiter.should_warn(a, after(3601)));
    }

    #[test]
    fn warn_limiter_stays_bounded() {
        let limiter = crate::api::WarnLimiter::default();
        let t0 = std::time::Instant::now();
        for _ in 0..25_000 {
            assert!(limiter.should_warn(uuid::Uuid::new_v4(), t0));
        }
        assert!(limiter.len() <= 10_000);
    }
}
