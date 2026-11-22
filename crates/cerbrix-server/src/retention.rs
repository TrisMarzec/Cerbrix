//! Deletes data that is older than its retention: problems resolved longer ago than
//! `retention.closed_problems_days`, with their notification records. Metric values are kept
//! by VictoriaMetrics and never deleted by the server.

use std::time::Duration;

use time::OffsetDateTime;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::config::RetentionConfig;
use crate::state::StateStore;

/// How often expired problems are deleted.
pub const RETENTION_EVERY: Duration = Duration::from_secs(3600);

/// Problems deleted by one statement, so that no single statement holds the table for long.
const BATCH: i64 = 5_000;

/// Deletes expired problems immediately and then every `every` until `shutdown` is
/// cancelled. Each run deletes in batches until a batch comes back short or shutdown is
/// requested. A failed run is logged and tried again at the next tick. With
/// `closed_problems_days == 0` nothing is deleted; the server then does not start the loop.
pub async fn run_retention_loop(
    store: StateStore,
    cfg: RetentionConfig,
    every: Duration,
    shutdown: CancellationToken,
) {
    if cfg.closed_problems_days == 0 {
        return;
    }
    let keep = time::Duration::days(i64::from(cfg.closed_problems_days));
    let mut ticker = tokio::time::interval(every.max(Duration::from_millis(1)));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = ticker.tick() => {}
        }
        let before = OffsetDateTime::now_utc() - keep;
        let mut total = 0u64;
        loop {
            let batch = tokio::select! {
                () = shutdown.cancelled() => break,
                batch = store.delete_problems_resolved_before(before, BATCH) => batch,
            };
            match batch {
                Ok(n) => {
                    total += n;
                    if n < BATCH as u64 {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "deleting expired problems failed");
                    break;
                }
            }
        }
        if total > 0 {
            tracing::info!(
                problems = total,
                days = cfg.closed_problems_days,
                "deleted problems resolved longer ago than the retention"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;
    use uuid::Uuid;

    /// One open problem and one resolved `resolved_days_ago` days ago, on a new host.
    async fn open_and_closed(pool: &PgPool, resolved_days_ago: i64) -> (Uuid, Uuid) {
        let host = Uuid::new_v4();
        sqlx::query("INSERT INTO hosts (id, name) VALUES ($1, 'h')")
            .bind(host)
            .execute(pool)
            .await
            .unwrap();
        let now = OffsetDateTime::now_utc();
        let (open, closed) = (Uuid::new_v4(), Uuid::new_v4());
        for (id, trigger, resolved) in [
            (open, "open", None),
            (
                closed,
                "closed",
                Some(now - time::Duration::days(resolved_days_ago)),
            ),
        ] {
            sqlx::query(
                "INSERT INTO problems (id, host_id, trigger_name, instance, severity, summary, \
                 started_at, resolved_at, resolve_reason) \
                 VALUES ($1, $2, $3, '', 'high', 's', $4, $5, \
                         CASE WHEN $5::timestamptz IS NULL THEN NULL ELSE 'condition_ended' END)",
            )
            .bind(id)
            .bind(host)
            .bind(trigger)
            .bind(now - time::Duration::days(resolved_days_ago + 1))
            .bind(resolved)
            .execute(pool)
            .await
            .unwrap();
        }
        (open, closed)
    }

    async fn problem_ids(pool: &PgPool) -> Vec<Uuid> {
        sqlx::query_scalar("SELECT id FROM problems ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    fn days(closed_problems_days: u32) -> RetentionConfig {
        RetentionConfig {
            closed_problems_days,
        }
    }

    async fn stop(shutdown: &CancellationToken, handle: tokio::task::JoinHandle<()>) {
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(30), handle)
            .await
            .expect("the loop stops after shutdown")
            .expect("the loop did not panic");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn the_loop_deletes_expired_problems(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let (open, _closed) = open_and_closed(&pool, 400).await;
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(run_retention_loop(
            store,
            days(365),
            Duration::from_millis(50),
            shutdown.clone(),
        ));
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while problem_ids(&pool).await != [open] {
            assert!(
                std::time::Instant::now() < deadline,
                "the expired problem is deleted"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        stop(&shutdown, handle).await;
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn zero_days_deletes_nothing(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let (open, closed) = open_and_closed(&pool, 4000).await;
        let mut before = vec![open, closed];
        before.sort();
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(run_retention_loop(
            store,
            days(0),
            Duration::from_millis(10),
            shutdown.clone(),
        ));
        // Several ticks would have run by now.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(problem_ids(&pool).await, before);
        stop(&shutdown, handle).await;
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn the_loop_stops_on_shutdown(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let (_open, closed) = open_and_closed(&pool, 10).await;
        let shutdown = CancellationToken::new();
        // The first tick runs at once; the next would come only after an hour.
        let handle = tokio::spawn(run_retention_loop(
            store,
            days(365),
            RETENTION_EVERY,
            shutdown.clone(),
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            problem_ids(&pool).await.contains(&closed),
            "a problem resolved within the retention is kept"
        );
        stop(&shutdown, handle).await;
    }
}
