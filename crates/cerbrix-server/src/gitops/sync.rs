//! One sync cycle (fetch, compile, apply) and the loop that repeats it.
//!
//! This is where "Out Of Sync" is decided: any cycle that cannot establish or apply
//! the repository's current revision records the reasons and leaves the active
//! configuration untouched.

use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::{ConfigSource, SourceError, SyncIssue, compile, format_issue, strip_control};
use crate::config::GitopsConfig;
use crate::state::{ApplyError, StateError, StateStore, SyncChanges};

/// What one call of [`sync_once`] did.
#[derive(Debug, Clone, PartialEq)]
// Short-lived value returned once per cycle; boxing would only complicate the public shape.
#[allow(clippy::large_enum_variant)]
pub enum SyncOutcome {
    /// A new revision was applied.
    Applied(SyncChanges),
    /// The repository is at the revision that is already active.
    Unchanged,
    /// The revision could not be applied; the previous configuration stays active.
    OutOfSync(Vec<SyncIssue>),
    /// Another instance holds the sync lock.
    Skipped,
}

/// Runs one sync cycle. The lock is taken here and released on every path.
pub async fn sync_once(
    store: &StateStore,
    source: &dyn ConfigSource,
    max_host_removal_percent: u8,
    now: OffsetDateTime,
) -> Result<SyncOutcome, StateError> {
    let Some(lock) = store.try_sync_lock().await? else {
        return Ok(SyncOutcome::Skipped);
    };
    let result = run_cycle(store, source, max_host_removal_percent, now).await;
    let released = lock.release().await;
    let outcome = result?;
    released?;
    Ok(outcome)
}

async fn fail(
    store: &StateStore,
    attempted_commit: Option<&str>,
    issues: Vec<SyncIssue>,
    now: OffsetDateTime,
) -> Result<SyncOutcome, StateError> {
    store
        .record_sync_failure(attempted_commit, &issues, now)
        .await?;
    Ok(SyncOutcome::OutOfSync(issues))
}

async fn run_cycle(
    store: &StateStore,
    source: &dyn ConfigSource,
    max_host_removal_percent: u8,
    now: OffsetDateTime,
) -> Result<SyncOutcome, StateError> {
    let snapshot = match source.fetch().await {
        Ok(snapshot) => snapshot,
        // No commit was established, so none is recorded as attempted.
        Err(SourceError::Fetch(reason)) => {
            return fail(store, None, vec![SyncIssue::new(None, reason)], now).await;
        }
        Err(SourceError::Invalid(issues)) => return fail(store, None, issues, now).await,
    };

    let state = store.sync_state().await?;
    if state.active_commit.as_deref() == Some(snapshot.commit.as_str()) {
        store.record_sync_unchanged(now).await?;
        return Ok(SyncOutcome::Unchanged);
    }

    // Compiling is CPU work that grows with the repository, so it runs on a blocking
    // thread instead of a runtime worker. A panic there ends this cycle, not the process.
    let commit = snapshot.commit.clone();
    let compiled = match tokio::task::spawn_blocking(move || compile(&snapshot)).await {
        Ok(Ok(compiled)) => compiled,
        Ok(Err(issues)) => return fail(store, Some(&commit), issues, now).await,
        Err(err) => {
            tracing::error!(%commit, %err, "compiling the configuration from Git failed");
            let reason = "the revision could not be compiled because of an internal error; \
                          the server log has the details";
            return fail(
                store,
                Some(&commit),
                vec![SyncIssue::new(None, reason)],
                now,
            )
            .await;
        }
    };

    match store
        .apply_config(&commit, &compiled, now, max_host_removal_percent)
        .await
    {
        Ok(changes) => Ok(SyncOutcome::Applied(changes)),
        Err(ApplyError::MassRemoval { removed, active }) => {
            let reason = format!(
                "this revision removes {removed} of {active} active hosts; \
                 raise gitops.max_host_removal_percent to apply it"
            );
            fail(
                store,
                Some(&commit),
                vec![SyncIssue::new(None, reason)],
                now,
            )
            .await
        }
        // Recorded, so that a revision the store rejects every time shows as Out Of Sync.
        // The database's text may contain values, so only its kind is recorded.
        Err(ApplyError::State(err)) => {
            tracing::error!(%commit, %err, "the configuration from Git could not be stored");
            let reason = format!(
                "the configuration could not be stored: {}",
                store_error_kind(&err)
            );
            fail(
                store,
                Some(&commit),
                vec![SyncIssue::new(None, reason)],
                now,
            )
            .await
        }
    }
}

/// A short description of a store error that is safe to record: the SQLSTATE code and
/// PostgreSQL's primary message (control characters removed, at most 200 characters),
/// or a fixed text for other kinds of error.
fn store_error_kind(err: &StateError) -> String {
    const MAX_MESSAGE_CHARS: usize = 200;
    match err {
        StateError::Db(sqlx::Error::Database(db)) => {
            let message: String = strip_control(db.message())
                .chars()
                .take(MAX_MESSAGE_CHARS)
                .collect();
            match db.code() {
                Some(code) => format!("SQLSTATE {code}, {message}"),
                None => message,
            }
        }
        StateError::Db(sqlx::Error::PoolTimedOut) => {
            "timed out waiting for a database connection".to_owned()
        }
        StateError::Db(sqlx::Error::Io(_)) => "the database connection failed".to_owned(),
        StateError::Db(sqlx::Error::Encode(_)) => {
            "a value could not be encoded for the database".to_owned()
        }
        StateError::Db(_) => "database error".to_owned(),
        StateError::Conflict(_) => "conflict".to_owned(),
        StateError::NotFound => "a record was not found".to_owned(),
    }
}

/// Runs a sync cycle immediately and then once per `interval_secs` (measured from the
/// start of the previous cycle) until `shutdown` is cancelled.
pub async fn run_sync_loop(
    store: StateStore,
    source: Arc<dyn ConfigSource>,
    cfg: GitopsConfig,
    shutdown: CancellationToken,
) {
    let period = Duration::from_secs(cfg.interval_secs.max(1));
    run_loop_every(
        store,
        source,
        cfg.max_host_removal_percent,
        period,
        shutdown,
    )
    .await;
}

async fn run_loop_every(
    store: StateStore,
    source: Arc<dyn ConfigSource>,
    max_host_removal_percent: u8,
    period: Duration,
    shutdown: CancellationToken,
) {
    let mut ticker = tokio::time::interval(period);
    // A cycle that overruns the interval delays the next one instead of causing a burst.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Logging state only: the issues of the previous cycle when it ended Out Of Sync.
    let mut previous_issues: Option<Vec<SyncIssue>> = None;

    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = ticker.tick() => {}
        }

        // A separate task, so that a panic in one cycle is caught and logged.
        let mut task = tokio::spawn({
            let store = store.clone();
            let source = Arc::clone(&source);
            let max = max_host_removal_percent;
            async move { sync_once(&store, source.as_ref(), max, OffsetDateTime::now_utc()).await }
        });
        let joined = tokio::select! {
            () = shutdown.cancelled() => {
                task.abort();
                // Wait until the aborted cycle is gone, so that its transaction and its
                // lock connection are dropped before the loop returns. The result is the
                // cancellation (or the cycle's own result if it had just finished).
                let _ = task.await;
                break;
            }
            joined = &mut task => joined,
        };

        match joined {
            Ok(Ok(SyncOutcome::Applied(changes))) => {
                previous_issues = None;
                let commit = store
                    .sync_state()
                    .await
                    .ok()
                    .and_then(|s| s.active_commit)
                    .unwrap_or_default();
                tracing::info!(
                    %commit,
                    hosts_added = changes.hosts.added.len(),
                    hosts_changed = changes.hosts.changed.len(),
                    hosts_removed = changes.hosts.removed.len(),
                    templates = changes.templates.added.len() + changes.templates.changed.len() + changes.templates.removed.len(),
                    notifications = changes.notifications.added.len() + changes.notifications.changed.len() + changes.notifications.removed.len(),
                    dashboards = changes.dashboards.added.len() + changes.dashboards.changed.len() + changes.dashboards.removed.len(),
                    displays = changes.displays.added.len() + changes.displays.changed.len() + changes.displays.removed.len(),
                    "applied configuration from Git"
                );
            }
            Ok(Ok(SyncOutcome::Unchanged)) => previous_issues = None,
            Ok(Ok(SyncOutcome::OutOfSync(issues))) => {
                if previous_issues.as_ref() != Some(&issues) {
                    let text: Vec<String> = issues.iter().map(format_issue).collect();
                    tracing::warn!(
                        count = issues.len(),
                        issues = %text.join("; "),
                        "configuration is out of sync with Git; the previous configuration stays active"
                    );
                }
                previous_issues = Some(issues);
            }
            Ok(Ok(SyncOutcome::Skipped)) => {}
            Ok(Err(err)) => tracing::error!(%err, "Git sync cycle failed"),
            Err(err) => tracing::error!(%err, "Git sync cycle panicked"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use sqlx::PgPool;
    use time::OffsetDateTime;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::config::GitopsConfig;
    use crate::gitops::{Snapshot, SourceError, SyncIssue, read_tree};
    use crate::state::{StateStore, SyncStatus};

    /// Returns queued results; once only one is left it is returned again.
    struct FakeSource {
        queue: Mutex<VecDeque<Result<Snapshot, SourceError>>>,
        calls: Mutex<usize>,
    }

    impl FakeSource {
        fn new(results: Vec<Result<Snapshot, SourceError>>) -> Self {
            Self {
                queue: Mutex::new(results.into()),
                calls: Mutex::new(0),
            }
        }
        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    fn copy_result(r: &Result<Snapshot, SourceError>) -> Result<Snapshot, SourceError> {
        match r {
            Ok(s) => Ok(s.clone()),
            Err(SourceError::Fetch(r)) => Err(SourceError::Fetch(r.clone())),
            Err(SourceError::Invalid(i)) => Err(SourceError::Invalid(i.clone())),
        }
    }

    #[async_trait]
    impl ConfigSource for FakeSource {
        async fn fetch(&self) -> Result<Snapshot, SourceError> {
            *self.calls.lock().unwrap() += 1;
            let mut q = self.queue.lock().unwrap();
            if q.len() > 1 {
                q.pop_front().unwrap()
            } else {
                copy_result(q.front().expect("queue is never empty"))
            }
        }
    }

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn fixture(commit: &str) -> Snapshot {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        read_tree(&dir, commit).unwrap()
    }

    fn broken(commit: &str) -> Snapshot {
        let mut s = fixture(commit);
        s.files
            .insert("hosts/web-01.json".to_owned(), b"{ not json".to_vec());
        s
    }

    fn with_extra_hosts(commit: &str, n: usize) -> Snapshot {
        let mut s = fixture(commit);
        for i in 0..n {
            let body = format!("{{ \"host\": \"extra-{i}\", \"address\": \"10.9.0.{i}\" }}");
            s.files
                .insert(format!("hosts/extra-{i}.json"), body.into_bytes());
        }
        s
    }

    fn at(secs: i64) -> OffsetDateTime {
        t0() + time::Duration::seconds(secs)
    }

    async fn cycle_at(store: &StateStore, source: &FakeSource, secs: i64) -> SyncOutcome {
        sync_once(store, source, 50, at(secs)).await.unwrap()
    }

    async fn cycle(store: &StateStore, source: &FakeSource) -> SyncOutcome {
        sync_once(store, source, 50, t0()).await.unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn valid_revision_is_applied(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let source = FakeSource::new(vec![Ok(fixture("commit-a"))]);
        let outcome = cycle(&store, &source).await;
        let SyncOutcome::Applied(changes) = outcome else {
            panic!("expected Applied");
        };
        assert_eq!(changes.hosts.added, ["db-01", "web-01"]);
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.status, SyncStatus::InSync);
        assert_eq!(state.active_commit.as_deref(), Some("commit-a"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn invalid_revision_changes_nothing(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let source = FakeSource::new(vec![Ok(fixture("commit-a")), Ok(broken("commit-b"))]);
        cycle(&store, &source).await;
        let outcome = cycle(&store, &source).await;
        let SyncOutcome::OutOfSync(issues) = outcome else {
            panic!("expected OutOfSync");
        };
        assert!(
            issues
                .iter()
                .any(|i| i.file.as_deref() == Some("hosts/web-01.json")),
            "{issues:?}"
        );
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.status, SyncStatus::OutOfSync);
        assert_eq!(state.active_commit.as_deref(), Some("commit-a"));
        assert_eq!(state.attempted_commit.as_deref(), Some("commit-b"));
        assert_eq!(state.issues, issues);
        let web = store.host_by_name("web-01").await.unwrap().unwrap();
        assert!(web.active);
        assert!(store.host_config(web.id).await.unwrap().is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn repeated_invalid_commit_adds_no_history(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let source = FakeSource::new(vec![Ok(fixture("commit-a")), Ok(broken("commit-b"))]);
        cycle_at(&store, &source, 0).await;
        let first = cycle_at(&store, &source, 10).await;
        let before = store.sync_state().await.unwrap();
        let second = cycle_at(&store, &source, 20).await;
        let third = cycle_at(&store, &source, 30).await;
        assert_eq!(first, second);
        assert_eq!(first, third);
        let after = store.sync_state().await.unwrap();
        assert_eq!(after.status, SyncStatus::OutOfSync);
        assert_eq!(after.issues, before.issues);
        assert_eq!(after.last_success_at, Some(at(0)));
        assert_eq!(after.last_attempt_at, Some(at(30)));
        assert_eq!(store.sync_history(100).await.unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn fixed_revision_clears_out_of_sync(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let source = FakeSource::new(vec![
            Ok(fixture("commit-a")),
            Ok(broken("commit-b")),
            Ok(fixture("commit-c")),
        ]);
        cycle(&store, &source).await;
        cycle(&store, &source).await;
        assert!(matches!(
            cycle(&store, &source).await,
            SyncOutcome::Applied(_)
        ));
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.status, SyncStatus::InSync);
        assert!(state.issues.is_empty());
        assert_eq!(state.active_commit.as_deref(), Some("commit-c"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn fetch_failure_then_same_commit_clears(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let source = FakeSource::new(vec![
            Ok(fixture("commit-a")),
            Err(SourceError::Fetch("unreachable".into())),
            Ok(fixture("commit-a")),
        ]);
        cycle_at(&store, &source, 0).await;
        let outcome = cycle_at(&store, &source, 10).await;
        let SyncOutcome::OutOfSync(issues) = outcome else {
            panic!("expected OutOfSync");
        };
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].file, None);
        assert!(issues[0].reason.contains("unreachable"));
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.status, SyncStatus::OutOfSync);
        assert_eq!(state.active_commit.as_deref(), Some("commit-a"));
        assert_eq!(state.attempted_commit, None);

        assert_eq!(cycle_at(&store, &source, 20).await, SyncOutcome::Unchanged);
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.status, SyncStatus::InSync);
        assert_eq!(state.last_success_at, Some(at(20)));
        assert!(state.issues.is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn invalid_from_source_is_recorded_without_commit(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let issue = SyncIssue {
            file: Some("hosts/x.json".into()),
            line: None,
            column: None,
            reason: "too big".into(),
        };
        let source = FakeSource::new(vec![Err(SourceError::Invalid(vec![issue.clone()]))]);
        assert_eq!(
            cycle(&store, &source).await,
            SyncOutcome::OutOfSync(vec![issue])
        );
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.status, SyncStatus::OutOfSync);
        assert_eq!(state.attempted_commit, None);
        assert_eq!(state.active_commit, None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn mass_removal_reports_reason(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let source = FakeSource::new(vec![
            Ok(with_extra_hosts("commit-a", 10)),
            Ok(fixture("commit-b")),
        ]);
        cycle(&store, &source).await;
        let outcome = cycle(&store, &source).await;
        let SyncOutcome::OutOfSync(issues) = outcome else {
            panic!("expected OutOfSync");
        };
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].file, None);
        assert_eq!(
            issues[0].reason,
            "this revision removes 10 of 12 active hosts; raise gitops.max_host_removal_percent to apply it"
        );
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.active_commit.as_deref(), Some("commit-a"));
        assert_eq!(state.attempted_commit.as_deref(), Some("commit-b"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn skipped_while_locked(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let source = FakeSource::new(vec![Ok(fixture("commit-a"))]);
        let lock = store.try_sync_lock().await.unwrap().unwrap();
        assert_eq!(cycle(&store, &source).await, SyncOutcome::Skipped);
        assert_eq!(source.calls(), 0);
        lock.release().await.unwrap();
        assert!(matches!(
            cycle(&store, &source).await,
            SyncOutcome::Applied(_)
        ));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn lock_is_released_after_failure(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let source = FakeSource::new(vec![Ok(broken("commit-b"))]);
        assert!(matches!(
            cycle(&store, &source).await,
            SyncOutcome::OutOfSync(_)
        ));
        let lock = store.try_sync_lock().await.unwrap();
        assert!(lock.is_some());
    }

    /// Polls until the installation is in sync, for at most 30 seconds.
    async fn wait_for_in_sync(store: &StateStore) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while tokio::time::Instant::now() < deadline {
            if store.sync_state().await.unwrap().status == SyncStatus::InSync {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// Polls until the sync lock can be taken, for at most 5 seconds. A lock whose
    /// connection was closed is released by PostgreSQL shortly afterwards, not at once.
    async fn wait_for_free_lock(store: &StateStore) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if let Some(lock) = store.try_sync_lock().await.unwrap() {
                lock.release().await.unwrap();
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn loop_stops_on_shutdown(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let source = Arc::new(FakeSource::new(vec![Ok(fixture("commit-a"))]));
        // An hour between cycles: only the immediate first cycle can run in this test,
        // however slowly the machine runs.
        let cfg = GitopsConfig {
            enabled: true,
            interval_secs: 3600,
            ..GitopsConfig::default()
        };
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(run_sync_loop(
            store.clone(),
            source.clone(),
            cfg,
            shutdown.clone(),
        ));
        assert!(
            wait_for_in_sync(&store).await,
            "the first cycle applied the revision"
        );
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("loop stops after shutdown")
            .unwrap();
        assert_eq!(source.calls(), 1);
        assert_eq!(
            store.sync_state().await.unwrap().active_commit.as_deref(),
            Some("commit-a")
        );
        assert!(
            wait_for_free_lock(&store).await,
            "the lock is free after the loop stopped"
        );
    }

    /// Panics on its first call, then serves the example snapshot.
    struct PanicOnce {
        calls: Mutex<usize>,
    }

    #[async_trait]
    impl ConfigSource for PanicOnce {
        async fn fetch(&self) -> Result<Snapshot, SourceError> {
            let first = {
                let mut calls = self.calls.lock().unwrap();
                *calls += 1;
                *calls == 1
            };
            assert!(!first, "simulated panic in a sync cycle");
            Ok(fixture("commit-a"))
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn loop_survives_a_panicking_cycle(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let source = Arc::new(PanicOnce {
            calls: Mutex::new(0),
        });
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(run_loop_every(
            store.clone(),
            source.clone(),
            50,
            Duration::from_millis(100),
            shutdown.clone(),
        ));
        let synced = wait_for_in_sync(&store).await;
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("loop stops")
            .expect("the loop itself did not panic");
        assert!(synced, "the cycle after the panic applied the revision");
        assert!(*source.calls.lock().unwrap() >= 2);
        assert!(
            wait_for_free_lock(&store).await,
            "the lock is free after the loop stopped"
        );
    }

    /// Makes every later apply fail inside its transaction: the apply writes the
    /// history, recording a failure does not.
    async fn break_history(pool: &PgPool) {
        sqlx::query("ALTER TABLE sync_history RENAME TO sync_history_gone")
            .execute(pool)
            .await
            .unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn lock_is_released_after_store_error(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        break_history(&pool).await;
        let source = FakeSource::new(vec![Ok(fixture("commit-a"))]);
        let result = sync_once(&store, &source, 50, t0()).await;
        assert!(
            matches!(result, Ok(SyncOutcome::OutOfSync(_))),
            "{result:?}"
        );
        assert_eq!(source.calls(), 1);
        let lock = store.try_sync_lock().await.unwrap();
        assert!(lock.is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn store_failure_is_recorded_as_out_of_sync(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let source = FakeSource::new(vec![Ok(fixture("commit-a")), Ok(fixture("commit-b"))]);
        assert!(matches!(
            cycle(&store, &source).await,
            SyncOutcome::Applied(_)
        ));
        break_history(&pool).await;

        let SyncOutcome::OutOfSync(issues) = cycle_at(&store, &source, 10).await else {
            panic!("expected OutOfSync");
        };
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].file, None);
        assert!(
            issues[0]
                .reason
                .starts_with("the configuration could not be stored: SQLSTATE 42P01"),
            "{issues:?}"
        );
        assert!(issues[0].reason.len() < 300, "{issues:?}");

        let state = store.sync_state().await.unwrap();
        assert_eq!(state.status, SyncStatus::OutOfSync);
        assert_eq!(state.active_commit.as_deref(), Some("commit-a"));
        assert_eq!(state.attempted_commit.as_deref(), Some("commit-b"));
        assert_eq!(state.last_attempt_at, Some(at(10)));
        assert_eq!(state.issues, issues);
        let lock = store.try_sync_lock().await.unwrap();
        assert!(lock.is_some(), "the lock is free after the failed cycle");
    }

    #[test]
    fn store_error_kind_records_no_detail() {
        assert_eq!(
            store_error_kind(&StateError::Conflict("secret value".into())),
            "conflict"
        );
        assert_eq!(
            store_error_kind(&StateError::Db(sqlx::Error::PoolTimedOut)),
            "timed out waiting for a database connection"
        );
    }
}
