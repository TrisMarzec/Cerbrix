//! Runs collectors on their own intervals and forwards their readings.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cerbrix_common::MetricValue;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::collector::{Collector, Reading};

type SharedCollector = Arc<Mutex<Box<dyn Collector>>>;

/// Drives a set of collectors, each on its own interval.
pub struct Scheduler {
    out: mpsc::Sender<MetricValue>,
    collectors: Vec<(SharedCollector, Duration)>,
}

impl Scheduler {
    pub fn new(out: mpsc::Sender<MetricValue>) -> Self {
        Self {
            out,
            collectors: Vec::new(),
        }
    }

    /// Registers a collector. It first runs immediately when `run` starts, then
    /// every `interval`.
    pub fn add(&mut self, collector: Box<dyn Collector>, interval: Duration) {
        self.collectors
            .push((Arc::new(Mutex::new(collector)), interval));
    }

    /// Runs until `shutdown` is cancelled or the output channel is closed.
    pub async fn run(self, shutdown: CancellationToken) {
        let stop = shutdown.child_token();
        let mut tasks = JoinSet::new();
        for (collector, interval) in self.collectors {
            tasks.spawn(run_collector(
                collector,
                interval,
                self.out.clone(),
                stop.clone(),
            ));
        }
        drop(self.out);
        // A task ends on shutdown or when the channel is closed; in the latter
        // case the remaining ones are stopped too.
        while tasks.join_next().await.is_some() {
            stop.cancel();
        }
    }
}

async fn run_collector(
    collector: SharedCollector,
    interval: Duration,
    out: mpsc::Sender<MetricValue>,
    stop: CancellationToken,
) {
    let name = lock(&collector).name();
    let mut ticker = tokio::time::interval_at(Instant::now(), interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let shared = Arc::clone(&collector);
        let result = tokio::task::spawn_blocking(move || lock(&shared).collect()).await;
        let readings = match result {
            Ok(Ok(readings)) => readings,
            Ok(Err(err)) => {
                tracing::warn!(collector = name, error = %err, "collector failed");
                continue;
            }
            Err(join_err) => {
                tracing::error!(collector = name, error = %join_err, "collector panicked");
                continue;
            }
        };
        let ts_ms = now_ms();
        for reading in readings {
            let Some(value) = into_metric(name, reading, ts_ms) else {
                continue;
            };
            // Waits while the channel is full (back-pressure).
            tokio::select! {
                () = stop.cancelled() => return,
                sent = out.send(value) => {
                    if sent.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// Locks the collector. A lock poisoned by an earlier panic is recovered so
/// the same collector is tried again at its next tick.
fn lock(collector: &SharedCollector) -> MutexGuard<'_, Box<dyn Collector>> {
    collector.lock().unwrap_or_else(PoisonError::into_inner)
}

fn into_metric(collector: &str, reading: Reading, ts_ms: i64) -> Option<MetricValue> {
    if !reading.value.is_finite() {
        tracing::warn!(collector, key = reading.key, "dropping non-finite reading");
        return None;
    }
    Some(MetricValue {
        key: reading.key.to_owned(),
        params: reading.params,
        ts_ms,
        value: reading.value,
    })
}

/// Unix milliseconds from the system clock.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::{Collector, Reading};
    use cerbrix_common::MetricValue;
    use std::collections::BTreeMap;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    enum Mode {
        Ok,
        Err,
        Panic,
        PanicOnceThenOk,
        NonFinite,
    }

    struct Fake {
        name: &'static str,
        key: &'static str,
        mode: Mode,
        calls: u32,
    }

    fn fake(name: &'static str, key: &'static str, mode: Mode) -> Box<dyn Collector> {
        Box::new(Fake {
            name,
            key,
            mode,
            calls: 0,
        })
    }

    impl Collector for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        fn collect(&mut self) -> anyhow::Result<Vec<Reading>> {
            self.calls += 1;
            let reading = |key, value| Reading {
                key,
                params: BTreeMap::new(),
                value,
            };
            match self.mode {
                Mode::Ok => Ok(vec![reading(self.key, 1.0)]),
                Mode::Err => anyhow::bail!("boom"),
                Mode::Panic => panic!("collector exploded"),
                Mode::PanicOnceThenOk if self.calls == 1 => panic!("collector exploded"),
                Mode::PanicOnceThenOk => Ok(vec![reading(self.key, 1.0)]),
                Mode::NonFinite => Ok(vec![
                    reading("test.nan", f64::NAN),
                    reading("test.inf", f64::INFINITY),
                    reading("test.good", 2.0),
                ]),
            }
        }
    }

    /// Awaits `n` values; `spawn_blocking` work is not driven by the paused
    /// clock, so waiting on the channel is what lets it finish.
    async fn recv_n(rx: &mut mpsc::Receiver<MetricValue>, n: usize) -> Vec<MetricValue> {
        let mut got = Vec::new();
        while got.len() < n {
            got.push(rx.recv().await.expect("channel closed early"));
        }
        got
    }

    fn count(values: &[MetricValue], key: &str) -> usize {
        values.iter().filter(|v| v.key == key).count()
    }

    #[tokio::test(start_paused = true)]
    async fn collectors_run_on_their_own_intervals() {
        let (tx, mut rx) = mpsc::channel(100);
        let mut sched = Scheduler::new(tx);
        sched.add(fake("a", "test.a", Mode::Ok), Duration::from_secs(10));
        sched.add(fake("b", "test.b", Mode::Ok), Duration::from_secs(30));
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(sched.run(shutdown.clone()));

        // t=0: both run immediately.
        let mut got = recv_n(&mut rx, 2).await;
        // Values due at t=10, 20, 30, 40, 50, 60 (B joins A at 30 and 60).
        for due in [1, 1, 2, 1, 1, 2] {
            tokio::time::advance(Duration::from_secs(10)).await;
            got.extend(recv_n(&mut rx, due).await);
        }
        assert_eq!(count(&got, "test.a"), 7);
        assert_eq!(count(&got, "test.b"), 3);
        shutdown.cancel();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn failing_collector_does_not_stop_others() {
        let (tx, mut rx) = mpsc::channel(100);
        let mut sched = Scheduler::new(tx);
        sched.add(fake("bad", "test.bad", Mode::Err), Duration::from_secs(10));
        sched.add(fake("good", "test.good", Mode::Ok), Duration::from_secs(10));
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(sched.run(shutdown.clone()));

        let mut got = recv_n(&mut rx, 1).await;
        tokio::time::advance(Duration::from_secs(10)).await;
        got.extend(recv_n(&mut rx, 1).await);
        assert!(got.iter().all(|v| v.key == "test.good"));
        shutdown.cancel();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn panicking_collector_does_not_stop_others() {
        let (tx, mut rx) = mpsc::channel(100);
        let mut sched = Scheduler::new(tx);
        sched.add(
            fake("panicky", "test.panicky", Mode::Panic),
            Duration::from_secs(10),
        );
        sched.add(
            fake("flaky", "test.flaky", Mode::PanicOnceThenOk),
            Duration::from_secs(10),
        );
        sched.add(fake("good", "test.good", Mode::Ok), Duration::from_secs(10));
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(sched.run(shutdown.clone()));

        // t=0: only "good" delivers (flaky panics on its first call).
        let mut got = recv_n(&mut rx, 1).await;
        // t=10: good and flaky (the same collector, retried) deliver.
        tokio::time::advance(Duration::from_secs(10)).await;
        got.extend(recv_n(&mut rx, 2).await);
        assert_eq!(count(&got, "test.good"), 2);
        assert_eq!(count(&got, "test.flaky"), 1);
        assert_eq!(count(&got, "test.panicky"), 0);
        shutdown.cancel();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn non_finite_readings_are_dropped() {
        let (tx, mut rx) = mpsc::channel(100);
        let mut sched = Scheduler::new(tx);
        sched.add(
            fake("nf", "test.nf", Mode::NonFinite),
            Duration::from_secs(10),
        );
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(sched.run(shutdown.clone()));

        let got = recv_n(&mut rx, 1).await;
        assert_eq!(got[0].key, "test.good");
        assert_eq!(got[0].value, 2.0);
        assert!(got[0].ts_ms > 1_600_000_000_000);
        shutdown.cancel();
        handle.await.unwrap();
        // Nothing but the finite reading was ever sent.
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn run_ends_on_shutdown() {
        let (tx, _rx) = mpsc::channel(100);
        let mut sched = Scheduler::new(tx);
        sched.add(fake("a", "test.a", Mode::Ok), Duration::from_secs(10));
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(sched.run(shutdown.clone()));
        tokio::time::advance(Duration::from_secs(25)).await;
        shutdown.cancel();
        handle.await.unwrap();
    }
}
