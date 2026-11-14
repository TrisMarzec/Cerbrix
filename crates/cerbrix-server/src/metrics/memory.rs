//! In-memory `MetricsStore` for tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use cerbrix_common::MetricValue;

use super::{MetricsError, MetricsStore, RangeQuery, Sample};

/// Keeps written values in memory and can be switched to fail.
#[derive(Debug, Default)]
pub struct MemoryMetrics {
    written: Mutex<Vec<(String, MetricValue)>>,
    failing: AtomicBool,
}

impl MemoryMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// While `failing` is true, `write` and `query_range` return
    /// `MetricsError::Unavailable`.
    pub fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    /// All written values with their host name, in write order.
    pub fn written(&self) -> Vec<(String, MetricValue)> {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<(String, MetricValue)>> {
        self.written.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn check(&self) -> Result<(), MetricsError> {
        if self.failing.load(Ordering::SeqCst) {
            Err(MetricsError::Unavailable("memory store set to fail".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl MetricsStore for MemoryMetrics {
    async fn write(&self, host: &str, values: &[MetricValue]) -> Result<(), MetricsError> {
        self.check()?;
        self.lock()
            .extend(values.iter().map(|v| (host.to_owned(), v.clone())));
        Ok(())
    }

    async fn query_range(&self, query: &RangeQuery) -> Result<Vec<Sample>, MetricsError> {
        self.check()?;
        let mut samples: Vec<Sample> = self
            .lock()
            .iter()
            .filter(|(host, v)| {
                *host == query.host
                    && v.key == query.key
                    && v.params == query.params
                    && (query.from_ms..=query.to_ms).contains(&v.ts_ms)
            })
            .map(|(_, v)| Sample {
                ts_ms: v.ts_ms,
                value: v.value,
            })
            .collect();
        samples.sort_by_key(|s| s.ts_ms);
        Ok(samples)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn mv(ts_ms: i64, value: f64) -> MetricValue {
        MetricValue {
            key: "cpu.usage".into(),
            params: BTreeMap::new(),
            ts_ms,
            value,
        }
    }

    fn query(host: &str) -> RangeQuery {
        RangeQuery {
            host: host.into(),
            key: "cpu.usage".into(),
            params: BTreeMap::new(),
            from_ms: 100,
            to_ms: 300,
            step_secs: 15,
        }
    }

    #[tokio::test]
    async fn query_returns_matching_samples_in_time_order() {
        let store = MemoryMetrics::new();
        store
            .write("a", &[mv(300, 3.0), mv(100, 1.0), mv(400, 4.0)])
            .await
            .unwrap();
        store.write("b", &[mv(200, 9.0)]).await.unwrap();
        let got = store.query_range(&query("a")).await.unwrap();
        assert_eq!(
            got,
            vec![
                Sample {
                    ts_ms: 100,
                    value: 1.0
                },
                Sample {
                    ts_ms: 300,
                    value: 3.0
                }
            ]
        );
        assert_eq!(store.written().len(), 4);
    }

    #[tokio::test]
    async fn failing_store_is_unavailable() {
        let store = MemoryMetrics::new();
        store.set_failing(true);
        assert!(matches!(
            store.write("a", &[mv(1, 1.0)]).await,
            Err(MetricsError::Unavailable(_))
        ));
        assert!(store.written().is_empty());
        store.set_failing(false);
        store.write("a", &[mv(1, 1.0)]).await.unwrap();
    }
}
