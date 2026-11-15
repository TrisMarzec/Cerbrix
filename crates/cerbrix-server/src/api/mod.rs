//! HTTP API. `agent` holds the routes the agents call.

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use cerbrix_common::ErrorBody;

use crate::config::Config;
use crate::metrics::MetricsStore;
use crate::state::StateStore;

mod agent;

/// Shared handles of the API. Everything in it is a connection handle or read-only
/// configuration; no request data is kept here.
#[derive(Clone)]
pub struct AppState {
    pub store: StateStore,
    pub metrics: Arc<dyn MetricsStore>,
    pub config: Arc<Config>,
}

/// Builds the router for the agent-facing API.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(agent::healthz))
        .route("/api/agent/v1/enroll", post(agent::enroll))
        .route("/api/agent/v1/values", post(agent::values))
        .route("/api/agent/v1/config", get(agent::config))
        .with_state(state)
}

/// An error answer: a status plus an `ErrorBody` with a stable code.
#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
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
            message: self.message.to_owned(),
        };
        (self.status, axum::Json(body)).into_response()
    }
}
