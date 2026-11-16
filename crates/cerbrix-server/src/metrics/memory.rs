//! In-memory `MetricsStore` for tests.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use cerbrix_common::{MetricValue, builtin_item};

use super::{LatestQuery, LatestSample, MetricsError, MetricsStore, RangeQuery, Sample};

/// Keeps written values in memory and can be switched to fail.
#[derive(Debug, Default)]
pub struct MemoryMetrics {
    written: Mutex<Vec<(String, MetricValue)>>,
    failing: AtomicBool,
    delay_ms: AtomicU64,
}

impl MemoryMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// While `failing` is true, `write`, `query_range` and `latest` return
    /// `MetricsError::Unavailable`.
    pub fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    /// Makes every call wait this long before it answers: a slow store.
    pub fn set_delay(&self, delay: std::time::Duration) {
        self.delay_ms.store(
            u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            Ordering::SeqCst,
        );
    }

    /// All written values with their host name, in write order.
    pub fn written(&self) -> Vec<(String, MetricValue)> {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<(String, MetricValue)>> {
        self.written.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn check(&self) -> Result<(), MetricsError> {
        let delay = self.delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
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
        self.check().await?;
        self.lock()
            .extend(values.iter().map(|v| (host.to_owned(), v.clone())));
        Ok(())
    }

    async fn query_range(&self, query: &RangeQuery) -> Result<Vec<Sample>, MetricsError> {
        self.check().await?;
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
    async fn latest(&self, query: &LatestQuery) -> Result<Vec<LatestSample>, MetricsError> {
        self.check().await?;
        if query.host.is_none() && query.key.is_none() {
            return Err(MetricsError::Rejected(
                "a latest query needs a host or a key".into(),
            ));
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let oldest = now_ms - i64::from(query.max_age_secs) * 1000;
        type SeriesId = (String, String, BTreeMap<String, String>);
        let mut newest: BTreeMap<SeriesId, LatestSample> = BTreeMap::new();
        for (host, v) in self.lock().iter() {
            if query.host.as_ref().is_some_and(|h| h != host)
                || query.key.as_ref().is_some_and(|k| *k != v.key)
                || builtin_item(&v.key).is_none()
                || v.ts_ms < oldest
            {
                continue;
            }
            let id = (host.clone(), v.key.clone(), v.params.clone());
            if newest.get(&id).is_none_or(|s| s.ts_ms < v.ts_ms) {
                newest.insert(
                    id,
                    LatestSample {
                        host: host.clone(),
                        key: v.key.clone(),
                        params: v.params.clone(),
                        ts_ms: v.ts_ms,
                        value: v.value,
                    },
                );
            }
        }
        Ok(newest.into_values().collect())
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

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    fn named(key: &str, ts_ms: i64, value: f64) -> MetricValue {
        MetricValue {
            key: key.into(),
            params: BTreeMap::new(),
            ts_ms,
            value,
        }
    }

    fn latest(host: Option<&str>, key: Option<&str>, max_age_secs: u32) -> LatestQuery {
        LatestQuery {
            host: host.map(str::to_owned),
            key: key.map(str::to_owned),
            max_age_secs,
        }
    }

    #[tokio::test]
    async fn latest_returns_newest_per_series() {
        let store = MemoryMetrics::new();
        let now = now_ms();
        let mount = |m: &str, ts, v| MetricValue {
            key: "disk.used_percent".into(),
            params: BTreeMap::from([("mount".to_owned(), m.to_owned())]),
            ts_ms: ts,
            value: v,
        };
        store
            .write(
                "a",
                &[
                    named("cpu.usage", now - 3000, 3.0),
                    named("cpu.usage", now - 1000, 1.0),
                    named("cpu.usage", now - 2000, 2.0),
                    mount("/", now - 5000, 50.0),
                    mount("/data", now - 4000, 60.0),
                ],
            )
            .await
            .unwrap();
        let mut got = store.latest(&latest(Some("a"), None, 600)).await.unwrap();
        got.sort_by(|x, y| (&x.key, &x.params).cmp(&(&y.key, &y.params)));
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].key, "cpu.usage");
        assert_eq!((got[0].ts_ms, got[0].value), (now - 1000, 1.0));
        assert_eq!(got[1].params["mount"], "/");
        assert_eq!(got[2].params["mount"], "/data");
        assert_eq!(got[2].host, "a");
    }

    #[tokio::test]
    async fn latest_respects_max_age() {
        let store = MemoryMetrics::new();
        let now = now_ms();
        store
            .write(
                "a",
                &[
                    named("cpu.usage", now - 30_000, 1.0),
                    named("memory.used_percent", now - 120_000, 2.0),
                ],
            )
            .await
            .unwrap();
        let got = store.latest(&latest(Some("a"), None, 60)).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "cpu.usage");
        let got = store.latest(&latest(Some("a"), None, 600)).await.unwrap();
        assert_eq!(got.len(), 2);
    }

    #[tokio::test]
    async fn latest_filters_by_host_and_key() {
        let store = MemoryMetrics::new();
        let now = now_ms();
        for host in ["a", "b"] {
            store
                .write(
                    host,
                    &[
                        named("cpu.usage", now - 1000, 1.0),
                        named("memory.used_percent", now - 1000, 2.0),
                        named("not.a.builtin", now - 1000, 3.0),
                    ],
                )
                .await
                .unwrap();
        }
        let both = store
            .latest(&latest(Some("a"), Some("cpu.usage"), 60))
            .await
            .unwrap();
        assert_eq!(both.len(), 1);
        assert_eq!(
            (both[0].host.as_str(), both[0].key.as_str()),
            ("a", "cpu.usage")
        );
        let host_only = store.latest(&latest(Some("b"), None, 60)).await.unwrap();
        assert_eq!(
            host_only.len(),
            2,
            "series that are no built-in item are skipped"
        );
        let key_only = store
            .latest(&latest(None, Some("memory.used_percent"), 60))
            .await
            .unwrap();
        assert_eq!(key_only.len(), 2);
        assert!(
            store.latest(&latest(None, None, 60)).await.is_err(),
            "a query over every series is refused"
        );
        store.set_failing(true);
        assert!(matches!(
            store.latest(&latest(Some("a"), None, 60)).await,
            Err(MetricsError::Unavailable(_))
        ));
    }
}
