//! PostgreSQL-backed state store: hosts, enrolment keys, agent credentials and unknown agents.
//!
//! Enrolment keys and agent credentials are stored only as SHA-256 hashes.

mod alerting;
mod enrollment;
mod hosts;
mod sync;
mod users;

pub use alerting::{
    AlertContext, LEASE_SECS, OldestPending, OutboxItem, OutboxSummary, Problem, ProblemEvent,
    ProblemFilter,
};
pub use enrollment::UnknownAgent;
pub use sync::{
    Applied, ApplyError, ChangeSet, HostDetails, SyncChanges, SyncHistoryEntry, SyncLock,
    SyncState, SyncStatus,
};
pub use users::User;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// A monitored host.
#[derive(Debug, Clone, PartialEq)]
pub struct Host {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
}

/// Errors of the state store.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found")]
    NotFound,
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Handle to the PostgreSQL state; cheap to clone.
#[derive(Clone)]
pub struct StateStore {
    pool: PgPool,
}

impl StateStore {
    /// Connects to the database and applies the embedded migrations.
    pub async fn connect(url: &str) -> Result<Self, StateError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(url)
            .await?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| StateError::Db(e.into()))?;
        Ok(Self { pool })
    }

    /// Wraps an existing pool (tests); does not run migrations.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Checks that the database answers.
    pub async fn ping(&self) -> Result<(), StateError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }
}

/// True when the error is a unique or primary key violation.
fn is_unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .is_some_and(|d| d.is_unique_violation())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;
    use time::{Duration, OffsetDateTime};

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_host_twice_conflicts(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let host = store.create_host("web-1").await.unwrap();
        assert_eq!(host.name, "web-1");
        assert!(host.active);
        assert!(matches!(
            store.create_host("web-1").await,
            Err(StateError::Conflict(_))
        ));
        // Names compare case-sensitively.
        store.create_host("WEB-1").await.unwrap();
        assert_eq!(store.host_by_name("web-1").await.unwrap(), Some(host));
        assert_eq!(store.host_by_name("nope").await.unwrap(), None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn key_is_stored_hashed(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let (id, plain) = store
            .create_enrollment_key("ci", now() + Duration::hours(1))
            .await
            .unwrap();
        assert!(plain.starts_with("cbx_ek_"));
        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT key_hash FROM enrollment_keys WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, crate::tokens::hash(&plain).to_vec());
        let leaked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM enrollment_keys WHERE description LIKE '%' || $1 || '%' \
             OR id::text LIKE '%' || $1 || '%'",
        )
        .bind(&plain)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(leaked, 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn key_validity(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let t0 = now();
        let expires = t0 + Duration::hours(1);
        let (_, plain) = store.create_enrollment_key("a", expires).await.unwrap();
        assert!(store.enrollment_key_valid(&plain, t0).await.unwrap());
        assert!(!store.enrollment_key_valid(&plain, expires).await.unwrap());
        assert!(
            !store
                .enrollment_key_valid("cbx_ek_unknown", t0)
                .await
                .unwrap()
        );

        let (id, plain2) = store.create_enrollment_key("b", expires).await.unwrap();
        assert!(store.enrollment_key_valid(&plain2, t0).await.unwrap());
        store.revoke_enrollment_key(id).await.unwrap();
        assert!(!store.enrollment_key_valid(&plain2, t0).await.unwrap());
        assert!(matches!(
            store.revoke_enrollment_key(uuid::Uuid::new_v4()).await,
            Err(StateError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn bind_twice_conflicts_and_release_allows_rebind(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let host = store.create_host("h").await.unwrap();
        let first = store.bind_credential(host.id).await.unwrap();
        assert!(first.starts_with("cbx_ac_"));
        assert!(matches!(
            store.bind_credential(host.id).await,
            Err(StateError::Conflict(_))
        ));
        store.release_binding(host.id).await.unwrap();
        assert!(store.host_for_credential(&first).await.unwrap().is_none());
        let second = store.bind_credential(host.id).await.unwrap();
        assert_ne!(first, second);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn bind_credential_refuses_inactive_host(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let host = store.create_host("h").await.unwrap();
        store.set_host_active(host.id, false).await.unwrap();
        assert!(matches!(
            store.bind_credential(host.id).await,
            Err(StateError::NotFound)
        ));
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_credentials")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn removal_wins_over_concurrent_bind(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let host = store.create_host("h").await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM hosts WHERE id = $1 FOR UPDATE")
            .bind(host.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("UPDATE hosts SET active = false WHERE id = $1")
            .bind(host.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("DELETE FROM agent_credentials WHERE host_id = $1")
            .bind(host.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        let binder = store.clone();
        let id = host.id;
        let bind = tokio::spawn(async move { binder.bind_credential(id).await });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!bind.is_finished(), "bind must wait for the row lock");
        tx.commit().await.unwrap();
        assert!(matches!(bind.await.unwrap(), Err(StateError::NotFound)));
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_credentials")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn concurrent_bind_yields_exactly_one_success(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let host = store.create_host("h").await.unwrap();
        let (a, b) = tokio::join!(
            store.bind_credential(host.id),
            store.bind_credential(host.id)
        );
        let oks = [&a, &b].iter().filter(|r| r.is_ok()).count();
        let conflicts = [&a, &b]
            .iter()
            .filter(|r| matches!(r, Err(StateError::Conflict(_))))
            .count();
        assert_eq!((oks, conflicts), (1, 1));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn credential_resolves_only_active_hosts(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let host = store.create_host("h").await.unwrap();
        let cred = store.bind_credential(host.id).await.unwrap();
        assert_eq!(
            store.host_for_credential(&cred).await.unwrap(),
            Some(host.clone())
        );
        store.set_host_active(host.id, false).await.unwrap();
        assert_eq!(store.host_for_credential(&cred).await.unwrap(), None);
        assert_eq!(store.host_for_credential("garbage").await.unwrap(), None);
        assert!(matches!(
            store.set_host_active(uuid::Uuid::new_v4(), true).await,
            Err(StateError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_agent_attempts_accumulate(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let t0 = now();
        let t1 = t0 + Duration::seconds(30);
        store.record_unknown_agent("ghost", t0).await.unwrap();
        store.record_unknown_agent("ghost", t1).await.unwrap();
        let list = store.unknown_agents().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].host_name, "ghost");
        assert_eq!(list[0].attempts, 2);
        assert!(list[0].first_seen < list[0].last_seen);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn ping_works(pool: PgPool) {
        StateStore::from_pool(pool).ping().await.unwrap();
    }
}
