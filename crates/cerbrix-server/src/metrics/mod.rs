//! Metrics storage: the `MetricsStore` interface and its backends.

use std::collections::BTreeMap;

use cerbrix_common::MetricValue;

mod victoria;
pub use victoria::VictoriaMetrics;

#[cfg(any(test, feature = "test-util"))]
mod memory;
#[cfg(any(test, feature = "test-util"))]
pub use memory::MemoryMetrics;

/// Storage for metric values, keyed by host, item key and item parameters.
#[async_trait::async_trait]
pub trait MetricsStore: Send + Sync {
    /// Stores `values` for `host`, each with its own measurement time.
    async fn write(&self, host: &str, values: &[MetricValue]) -> Result<(), MetricsError>;
    /// Returns the samples of one series within the time range, in time order.
    async fn query_range(&self, query: &RangeQuery) -> Result<Vec<Sample>, MetricsError>;
}

/// A range query for one series (host, item key and item parameters).
#[derive(Debug, Clone, PartialEq)]
pub struct RangeQuery {
    pub host: String,
    pub key: String,
    pub params: BTreeMap<String, String>,
    /// Start of the range in Unix milliseconds.
    pub from_ms: i64,
    /// End of the range in Unix milliseconds.
    pub to_ms: i64,
    /// Resolution step in seconds.
    pub step_secs: u32,
}

/// One stored data point.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// Measurement time in Unix milliseconds.
    pub ts_ms: i64,
    pub value: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    /// The store could not be reached or failed (connection, timeout, 5xx).
    /// Retrying later may succeed.
    #[error("metrics store unavailable: {0}")]
    Unavailable(String),
    /// The store refused the request (4xx or an unusable response). Retrying
    /// the same request will not help.
    #[error("metrics store rejected the request: {0}")]
    Rejected(String),
}
