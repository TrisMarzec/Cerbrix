//! Delivers queued notifications through a channel, with retries.
//!
//! The queue is the `notification_outbox` table: `claim_notifications` hands each row to one
//! caller only (a lease), so several server instances do not work on the same message at
//! once, and it holds a `resolved` row back while its `started` row is still pending, so the
//! end of a problem is never mailed before its start. A row that could not be sent stays in
//! the table and is tried again later; an alert survives a mail server that is down for a
//! while.
//!
//! Delivery is at least once, not exactly once. A message is normally sent once, but it is
//! sent again when the server stops or loses its database between the mail server's
//! acceptance and `mark_notification_sent`, when that mark fails, or when the timeout ends a
//! send after the mail server had already accepted the message: the lease ends and the row
//! is claimed again. Duplicates are possible in those cases; losses are not.

use std::sync::Arc;
use std::time::{Duration, Instant};

use time::OffsetDateTime;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::channel::{Channel, ChannelError};
use crate::config::MAX_SMTP_TIMEOUT_SECS;
use crate::state::{LEASE_SECS, OutboxItem, StateError, StateStore};

/// A message that keeps failing with a retryable error is given up after this long.
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(24 * 3600);

/// A message that keeps failing with a permanent error is given up after this long. It is
/// retried until then, because a misconfigured relay can answer 5xx for every message and an
/// alert must not be lost to that within seconds.
pub const GIVE_UP_PERMANENT_AFTER: Duration = Duration::from_secs(3600);

/// Safety margin between the end of the last send of a batch and the end of the lease.
const LEASE_MARGIN_SECS: u64 = 10;

/// A batch starts no send later than this long after the claim. The last send it starts can
/// take up to `MAX_SMTP_TIMEOUT_SECS`, and everything must end before the lease does,
/// otherwise rows still waiting in the batch could be claimed by another instance while this
/// one is working through it. With a 300 s lease and a 120 s timeout this is 170 s.
const BATCH_BUDGET: Duration =
    Duration::from_secs(LEASE_SECS as u64 - MAX_SMTP_TIMEOUT_SECS - LEASE_MARGIN_SECS);

const _: () = assert!(LEASE_SECS > 0);
const _: () = assert!(BATCH_BUDGET.as_secs() > 0);
const _: () = assert!(BATCH_BUDGET.as_secs() + MAX_SMTP_TIMEOUT_SECS < LEASE_SECS as u64);

/// Notifications claimed per cycle.
const BATCH: i64 = 20;

/// Longest wait between two attempts.
const MAX_DELAY_SECS: u64 = 3600;

/// Wait before the next attempt; `attempts` counts the attempt just made (1 after the first
/// try): 1 minute, 2, 4, ... capped at one hour.
pub fn retry_delay(attempts: i32) -> Duration {
    let exponent = u32::try_from(attempts.max(1) - 1).unwrap_or(0).min(12);
    Duration::from_secs((60u64 << exponent).min(MAX_DELAY_SECS))
}

fn age_reaches(now: OffsetDateTime, created_at: OffsetDateTime, limit: Duration) -> bool {
    now - created_at >= time::Duration::try_from(limit).unwrap_or(time::Duration::MAX)
}

/// Claims due notifications and sends them one after another, returning how many were sent.
///
/// An error from one notification does not stop the rest, and a store error while marking is
/// logged (the lease expires and the row is tried again). Only a failing claim is an error.
/// The batch stops after [`BATCH_BUDGET`]; see `dispatch_with_budget`.
pub async fn dispatch_once(
    store: &StateStore,
    channel: &dyn Channel,
    now: OffsetDateTime,
) -> Result<usize, StateError> {
    dispatch_with_budget(store, channel, now, BATCH_BUDGET).await
}

/// `dispatch_once` with the time budget as a parameter. Once `budget` has passed since the
/// claim, the rest of the batch is left alone: those rows stay leased and become claimable
/// when their lease ends, so a slow batch (a blackholed mail server) never outlives its lease
/// and lets a second instance send the same rows.
async fn dispatch_with_budget(
    store: &StateStore,
    channel: &dyn Channel,
    now: OffsetDateTime,
    budget: Duration,
) -> Result<usize, StateError> {
    let items = store.claim_notifications(now, BATCH).await?;
    let cycle_start = Instant::now();
    let mut sent = 0;
    let total = items.len();
    for (done, item) in items.into_iter().enumerate() {
        if cycle_start.elapsed() >= budget {
            tracing::warn!(
                left = total - done,
                "notification batch used up its time budget; the rest waits for its lease to end"
            );
            break;
        }
        let result = channel.send(&item.recipient, &item.notification).await;
        // Sends can take minutes: the marks carry the time since the claim.
        let after = now + cycle_start.elapsed();
        if record_outcome(store, channel, &item, result, after).await {
            sent += 1;
        }
    }
    Ok(sent)
}

/// Records the outcome of one send; true when the message was sent.
async fn record_outcome(
    store: &StateStore,
    channel: &dyn Channel,
    item: &OutboxItem,
    result: Result<(), ChannelError>,
    now: OffsetDateTime,
) -> bool {
    let n = &item.notification;
    let marked = match result {
        Ok(()) => {
            tracing::info!(
                channel = channel.name(),
                host = %n.host,
                trigger = %n.trigger,
                event = n.kind.as_str(),
                recipient = %item.recipient,
                "notification sent"
            );
            if let Err(err) = store.mark_notification_sent(item.id, now).await {
                // The mail is out; the lease lets the row come back, which can send it again.
                tracing::error!(id = item.id, %err, "cannot mark a sent notification as sent");
            }
            return true;
        }
        Err(ChannelError::Retryable(error)) => {
            if age_reaches(now, item.created_at, GIVE_UP_AFTER) {
                tracing::error!(
                    id = item.id, trigger = %n.trigger, event = n.kind.as_str(), %error,
                    "notification given up after 24 hours of failed attempts"
                );
                store.mark_notification_failed(item.id, &error, now).await
            } else {
                tracing::warn!(
                    id = item.id, attempts = item.attempts, trigger = %n.trigger,
                    event = n.kind.as_str(), %error, "notification not sent; will retry"
                );
                retry(store, item, &error, now).await
            }
        }
        Err(ChannelError::Permanent(error)) => {
            if age_reaches(now, item.created_at, GIVE_UP_PERMANENT_AFTER) {
                tracing::error!(
                    id = item.id, trigger = %n.trigger, event = n.kind.as_str(), %error,
                    "notification given up after one hour of permanent errors"
                );
                store.mark_notification_failed(item.id, &error, now).await
            } else {
                tracing::error!(
                    id = item.id, attempts = item.attempts, trigger = %n.trigger,
                    event = n.kind.as_str(), %error,
                    "permanent error sending a notification; it will be retried and given up one hour after it was created"
                );
                retry(store, item, &error, now).await
            }
        }
    };
    if let Err(err) = marked {
        tracing::error!(id = item.id, %err, "cannot record the outcome of a notification");
    }
    false
}

async fn retry(
    store: &StateStore,
    item: &OutboxItem,
    error: &str,
    now: OffsetDateTime,
) -> Result<(), StateError> {
    let next = now + retry_delay(item.attempts);
    store.mark_notification_retry(item.id, error, next).await
}

/// Runs a dispatch immediately and then once per `every` until `shutdown` is cancelled.
pub async fn run_dispatch_loop(
    store: StateStore,
    channel: Arc<dyn Channel>,
    every: Duration,
    shutdown: CancellationToken,
) {
    let mut ticker = tokio::time::interval(every.max(Duration::from_millis(1)));
    // A cycle that overruns the interval delays the next one instead of causing a burst.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let store = Arc::new(store);

    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = ticker.tick() => {}
        }

        // A separate task, so that a panic in one cycle is caught and logged.
        let mut task = tokio::spawn({
            let store = Arc::clone(&store);
            let channel = Arc::clone(&channel);
            async move { dispatch_once(&store, channel.as_ref(), OffsetDateTime::now_utc()).await }
        });
        let joined = tokio::select! {
            () = shutdown.cancelled() => {
                // The aborted send is dropped; its row stays leased and is tried again
                // after the lease ends.
                task.abort();
                let _ = task.await;
                break;
            }
            joined = &mut task => joined,
        };
        match joined {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => tracing::error!(%err, "notification dispatch failed"),
            Err(err) => tracing::error!(%err, "notification dispatch panicked"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use cerbrix_common::config::Severity;
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::*;
    use crate::alerting::{Evaluation, EventKind, Notification, OpenProblem, ResolveProblem};
    use crate::gitops::{compile, read_tree};

    /// Records sends; answers from a script, then with `default`.
    struct FakeChannel {
        script: Mutex<VecDeque<Result<(), ChannelError>>>,
        default: Mutex<Result<(), ChannelError>>,
        sent: Mutex<Vec<(String, EventKind)>>,
        panic_once: AtomicBool,
        panicked: AtomicBool,
        /// Milliseconds each send sleeps.
        delay: std::sync::atomic::AtomicU64,
    }

    impl FakeChannel {
        fn new() -> Self {
            Self {
                script: Mutex::default(),
                default: Mutex::new(Ok(())),
                sent: Mutex::default(),
                panic_once: AtomicBool::new(false),
                panicked: AtomicBool::new(false),
                delay: std::sync::atomic::AtomicU64::new(0),
            }
        }

        fn failing(error: ChannelError) -> Self {
            let c = Self::new();
            c.set_default(Err(error));
            c
        }

        fn set_default(&self, result: Result<(), ChannelError>) {
            *self.default.lock().unwrap() = result;
        }

        fn sent(&self) -> Vec<(String, EventKind)> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Channel for FakeChannel {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn send(&self, to: &str, n: &Notification) -> Result<(), ChannelError> {
            if self.panic_once.swap(false, Ordering::SeqCst) {
                self.panicked.store(true, Ordering::SeqCst);
                panic!("scripted panic in a channel");
            }
            let delay = self.delay.load(Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            let result = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| self.default.lock().unwrap().clone());
            if result.is_ok() {
                self.sent.lock().unwrap().push((to.to_owned(), n.kind));
            }
            result
        }
    }

    fn at(ms: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).unwrap()
    }

    fn now_ms() -> i64 {
        i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap()
    }

    const T0_MS: i64 = 1_800_000_000_000;

    async fn setup(pool: PgPool) -> (StateStore, Uuid) {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        let compiled = compile(&read_tree(&dir, "c1").unwrap()).unwrap();
        let store = StateStore::from_pool(pool);
        store
            .apply_config("c1", &compiled, at(T0_MS), 50, false)
            .await
            .unwrap();
        let host = store.host_by_name("web-01").await.unwrap().unwrap();
        (store, host.id)
    }

    async fn open_problem(store: &StateStore, host: Uuid, trigger: &str, at_ms: i64) {
        store
            .evaluate_host(host, true, |_| Evaluation {
                open: vec![OpenProblem {
                    trigger: trigger.to_owned(),
                    instance: String::new(),
                    severity: Severity::Critical,
                    at_ms,
                    summary: format!("{trigger} is bad"),
                }],
                ..Evaluation::default()
            })
            .await
            .unwrap();
    }

    async fn resolve_problem(store: &StateStore, host: Uuid, trigger: &str, at_ms: i64) {
        store
            .evaluate_host(host, true, |_| Evaluation {
                resolve: vec![ResolveProblem {
                    trigger: trigger.to_owned(),
                    instance: String::new(),
                    at_ms,
                }],
                ..Evaluation::default()
            })
            .await
            .unwrap();
    }

    /// Counts outbox rows matching a literal SQL condition written by the test.
    async fn count(pool: &PgPool, filter: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM notification_outbox WHERE {filter}"
        )))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn wait_for<F, Fut>(mut condition: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !condition().await {
            assert!(Instant::now() < deadline, "condition not met in time");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[test]
    fn retry_delay_doubles_and_caps() {
        assert_eq!(retry_delay(1), Duration::from_secs(60));
        assert_eq!(retry_delay(2), Duration::from_secs(120));
        assert_eq!(retry_delay(3), Duration::from_secs(240));
        assert_eq!(retry_delay(7), Duration::from_secs(3600));
        assert_eq!(retry_delay(30), Duration::from_secs(3600));
        assert_eq!(retry_delay(i32::MAX), Duration::from_secs(3600));
        assert_eq!(retry_delay(0), Duration::from_secs(60));
        assert_eq!(retry_delay(-5), Duration::from_secs(60));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn sends_and_marks_sent(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        open_problem(&store, host, "Disk", T0_MS).await;
        let total = count(&pool, "true").await;
        assert!(total > 0);
        let channel = FakeChannel::new();
        let now = at(T0_MS);
        let sent = dispatch_once(&store, &channel, now).await.unwrap();
        assert_eq!(i64::try_from(sent).unwrap(), total);
        assert_eq!(channel.sent().len(), sent);
        assert_eq!(count(&pool, "sent_at IS NOT NULL").await, total);
        // Nothing is sent a second time, not even after the lease would have ended.
        let later = now + time::Duration::hours(1);
        assert_eq!(dispatch_once(&store, &channel, later).await.unwrap(), 0);
        assert_eq!(channel.sent().len(), sent);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn retryable_schedules_a_retry(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        open_problem(&store, host, "Disk", T0_MS).await;
        let total = count(&pool, "true").await;
        let channel = FakeChannel::failing(ChannelError::Retryable("down".into()));
        let now = at(T0_MS);
        assert_eq!(dispatch_once(&store, &channel, now).await.unwrap(), 0);
        assert_eq!(
            count(&pool, "sent_at IS NULL AND failed_at IS NULL").await,
            total
        );
        assert_eq!(
            count(&pool, "last_error = 'down' AND locked_until IS NULL").await,
            total
        );

        channel.set_default(Ok(()));
        // Not claimed again before the delay (first retry: one minute) ...
        let early = now + time::Duration::seconds(30);
        assert_eq!(dispatch_once(&store, &channel, early).await.unwrap(), 0);
        assert!(channel.sent().is_empty());
        // ... and claimed after it.
        let due = now + time::Duration::seconds(61);
        let sent = dispatch_once(&store, &channel, due).await.unwrap();
        assert_eq!(i64::try_from(sent).unwrap(), total);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn permanent_is_retried_for_an_hour_then_given_up(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        open_problem(&store, host, "Disk", T0_MS).await;
        let total = count(&pool, "true").await;
        let channel = FakeChannel::failing(ChannelError::Permanent("550 no".into()));
        let now = at(T0_MS);
        assert_eq!(dispatch_once(&store, &channel, now).await.unwrap(), 0);
        // A fresh message is scheduled for a retry, not failed.
        assert_eq!(
            count(&pool, "failed_at IS NULL AND sent_at IS NULL").await,
            total
        );
        assert_eq!(count(&pool, "last_error = '550 no'").await, total);

        // 61 minutes after it was created it is given up.
        let late = now + time::Duration::minutes(61);
        assert_eq!(dispatch_once(&store, &channel, late).await.unwrap(), 0);
        assert_eq!(count(&pool, "failed_at IS NOT NULL").await, total);
        let much_later = late + time::Duration::hours(2);
        assert_eq!(
            dispatch_once(&store, &channel, much_later).await.unwrap(),
            0
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn gives_up_after_24_hours(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        open_problem(&store, host, "Disk", T0_MS).await;
        let total = count(&pool, "true").await;
        let channel = FakeChannel::failing(ChannelError::Retryable("down".into()));
        // Still retried at 23 hours, failed at 25.
        let almost = at(T0_MS) + time::Duration::hours(23);
        dispatch_once(&store, &channel, almost).await.unwrap();
        assert_eq!(count(&pool, "failed_at IS NOT NULL").await, 0);
        let late = at(T0_MS) + time::Duration::hours(25);
        dispatch_once(&store, &channel, late).await.unwrap();
        assert_eq!(count(&pool, "failed_at IS NOT NULL").await, total);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn mail_server_outage_delivers_in_order_afterwards(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        open_problem(&store, host, "Disk", T0_MS).await;
        resolve_problem(&store, host, "Disk", T0_MS + 60_000).await;
        let channel = FakeChannel::failing(ChannelError::Retryable("down".into()));
        let mut now = at(T0_MS + 120_000);
        // The outage: the resolved mail is not even tried while the started one is pending.
        for _ in 0..3 {
            assert_eq!(dispatch_once(&store, &channel, now).await.unwrap(), 0);
            now += time::Duration::hours(1);
        }
        assert!(channel.sent().is_empty());

        channel.set_default(Ok(()));
        now += time::Duration::hours(1);
        let mut rounds = 0;
        while count(&pool, "sent_at IS NULL AND failed_at IS NULL").await > 0 {
            dispatch_once(&store, &channel, now).await.unwrap();
            now += time::Duration::minutes(5);
            rounds += 1;
            assert!(rounds < 10);
        }
        let sent = channel.sent();
        assert!(!sent.is_empty());
        let mut recipients: Vec<&String> = sent.iter().map(|(to, _)| to).collect();
        recipients.sort();
        recipients.dedup();
        for to in recipients {
            let kinds: Vec<EventKind> = sent
                .iter()
                .filter(|(r, _)| r == to)
                .map(|(_, k)| *k)
                .collect();
            assert_eq!(kinds, [EventKind::Started, EventKind::Resolved], "for {to}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn two_dispatchers_do_not_send_twice(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        for trigger in ["Disk", "Memory", "Load", "Swap"] {
            open_problem(&store, host, trigger, T0_MS).await;
        }
        let total = count(&pool, "true").await;
        assert!(total >= 3, "the example config yields {total} rows");
        let (a, b) = (FakeChannel::new(), FakeChannel::new());
        let now = at(T0_MS);
        let (ra, rb) = tokio::join!(
            dispatch_once(&store, &a, now),
            dispatch_once(&store, &b, now)
        );
        let sent = ra.unwrap() + rb.unwrap();
        assert_eq!(i64::try_from(sent).unwrap(), total);
        assert_eq!(a.sent().len() + b.sent().len(), sent);
        assert_eq!(count(&pool, "sent_at IS NOT NULL").await, total);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_failing_message_does_not_stop_the_batch(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        for trigger in ["Disk", "Memory"] {
            open_problem(&store, host, trigger, T0_MS).await;
        }
        let total = count(&pool, "true").await;
        let channel = FakeChannel::new();
        channel
            .script
            .lock()
            .unwrap()
            .push_back(Err(ChannelError::Retryable("first fails".into())));
        let sent = dispatch_once(&store, &channel, at(T0_MS)).await.unwrap();
        assert_eq!(i64::try_from(sent).unwrap(), total - 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn slow_batch_stops_before_the_lease_ends(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        for trigger in ["Disk", "Memory", "Load", "Swap"] {
            open_problem(&store, host, trigger, T0_MS).await;
        }
        let total = count(&pool, "true").await;
        assert!(total >= 3);
        let channel = FakeChannel::new();
        // Each send takes at least one second, so after two sends at least two seconds have
        // passed and the 1.5 s budget is used up whatever the machine: at most two are sent.
        // The first send starts right after the claim; only a stall of 1.5 s before it could
        // leave nothing sent.
        channel.delay.store(1000, Ordering::SeqCst);
        let sent = dispatch_with_budget(&store, &channel, at(T0_MS), Duration::from_millis(1500))
            .await
            .unwrap();
        assert!(
            sent >= 1 && i64::try_from(sent).unwrap() < total,
            "sent {sent}"
        );
        assert_eq!(
            count(&pool, "sent_at IS NOT NULL").await,
            i64::try_from(sent).unwrap()
        );
        // The rest is untouched: no error, not failed, still leased for its claimer.
        let untouched = count(
            &pool,
            "sent_at IS NULL AND failed_at IS NULL AND last_error IS NULL AND locked_until IS NOT NULL",
        )
        .await;
        assert_eq!(untouched, total - i64::try_from(sent).unwrap());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn marks_carry_the_time_spent_on_earlier_messages(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        open_problem(&store, host, "Disk", T0_MS).await;
        open_problem(&store, host, "Memory", T0_MS).await;
        let channel = FakeChannel::new();
        channel.delay.store(200, Ordering::SeqCst);
        dispatch_once(&store, &channel, at(T0_MS)).await.unwrap();
        let times: Vec<OffsetDateTime> =
            sqlx::query_scalar("SELECT sent_at FROM notification_outbox ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(times.len() >= 2);
        let spread = *times.last().unwrap() - times[0];
        // The last of n messages waited for n - 1 sends of 200 ms.
        let n = i32::try_from(times.len()).unwrap();
        assert!(
            spread >= time::Duration::milliseconds(200 * i64::from(n - 1)),
            "{spread}"
        );
    }
    /// Rows with times around the real clock, for tests that run the loop.
    async fn live_store(pool: PgPool) -> (StateStore, i64) {
        let (store, host) = setup(pool.clone()).await;
        let t = now_ms() - 600_000;
        open_problem(&store, host, "Disk", t).await;
        let total = count(&pool, "true").await;
        (store, total)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn loop_delivers_and_stops_on_shutdown(pool: PgPool) {
        let (store, total) = live_store(pool.clone()).await;
        let channel = Arc::new(FakeChannel::new());
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(run_dispatch_loop(
            store,
            channel.clone(),
            Duration::from_millis(50),
            shutdown.clone(),
        ));
        wait_for(|| async { count(&pool, "sent_at IS NOT NULL").await == total }).await;
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("loop stops")
            .unwrap();
        assert_eq!(i64::try_from(channel.sent().len()).unwrap(), total);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn loop_survives_a_failing_cycle(pool: PgPool) {
        let (store, total) = live_store(pool.clone()).await;
        let channel = Arc::new(FakeChannel::new());
        channel.panic_once.store(true, Ordering::SeqCst);
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(run_dispatch_loop(
            store,
            channel.clone(),
            Duration::from_millis(50),
            shutdown.clone(),
        ));
        wait_for(|| async { channel.panicked.load(Ordering::SeqCst) }).await;
        // The panicked cycle left its rows leased; end the lease as time would.
        sqlx::query("UPDATE notification_outbox SET locked_until = NULL")
            .execute(&pool)
            .await
            .unwrap();
        wait_for(|| async { count(&pool, "sent_at IS NOT NULL").await == total }).await;
        assert!(!handle.is_finished(), "the loop must survive the panic");
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("loop stops")
            .unwrap();
    }
}
