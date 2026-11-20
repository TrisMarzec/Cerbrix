//! HTTP API. `agent` holds the routes the agents call.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use cerbrix_common::ErrorBody;
use uuid::Uuid;

use crate::auth::PasswordGate;
use crate::auth::throttle::LoginThrottle;
use crate::config::Config;
use crate::metrics::MetricsStore;
use crate::state::StateStore;

mod agent;
pub mod ui;

/// Shared handles of the API. Everything in it is a connection handle or read-only
/// configuration; no request data is kept here. There are two exceptions: `warnings`, which
/// only limits how often a log line is written (no decision depends on it), and `logins`, the
/// failed logins per client address.
#[derive(Clone)]
pub struct AppState {
    pub store: StateStore,
    pub metrics: Arc<dyn MetricsStore>,
    pub config: Arc<Config>,
    pub warnings: Arc<WarnLimiter>,
    /// Whether problems queue e-mail: true when an SMTP server is configured.
    pub queue_mail: bool,
    /// The limit on password hashing and checking; one for the whole process.
    pub passwords: PasswordGate,
    /// Failed logins per client address; one for the whole process.
    pub logins: Arc<LoginThrottle>,
}

/// Lets a warning through at most once per hour per host. Process-local and bounded: it
/// is forgotten on restart and cleared when it holds more than `MAX_ENTRIES` hosts, which
/// at worst repeats a log line.
#[derive(Debug, Default)]
pub struct WarnLimiter {
    last: Mutex<HashMap<Uuid, Instant>>,
}

impl WarnLimiter {
    const INTERVAL: Duration = Duration::from_secs(3600);
    const MAX_ENTRIES: usize = 10_000;

    /// True when the warning for `host` may be logged at `now`; records it if so.
    pub fn should_warn(&self, host: Uuid, now: Instant) -> bool {
        // The lock is never held across an await: this function is synchronous.
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(&at) = last.get(&host)
            && now.saturating_duration_since(at) < Self::INTERVAL
        {
            return false;
        }
        if last.len() >= Self::MAX_ENTRIES {
            last.clear();
        }
        last.insert(host, now);
        true
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// Builds the router: the agent-facing API and the UI API under `/api/v1`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(agent::healthz))
        .route("/api/agent/v1/enroll", post(agent::enroll))
        .route("/api/agent/v1/values", post(agent::values))
        .route("/api/agent/v1/config", get(agent::config))
        .with_state(state.clone())
        .merge(ui::router(state))
        // Last: a fallback that answers pages and never claims an API path.
        .merge(crate::webui::router())
}

/// An error answer: a status plus an `ErrorBody` with a stable code.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: std::borrow::Cow<'static, str>,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// An error with a message made at run time; it must not contain secrets or raw input
    /// beyond names that were validated.
    pub(crate) fn with_message(status: StatusCode, code: &'static str, message: String) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// A generic 500; the cause is logged by the caller, never sent to the client.
    pub(crate) fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: self.code.to_owned(),
            message: self.message.into_owned(),
        };
        (self.status, axum::Json(body)).into_response()
    }
}
