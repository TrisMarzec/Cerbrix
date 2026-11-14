//! Applying a compiled configuration, sync state, history and the sync lock.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sqlx::pool::PoolConnection;
use sqlx::types::Json;
use sqlx::{Postgres, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{StateError, StateStore};
use crate::gitops::{Compiled, EffectiveHost, SyncIssue};

/// Advisory lock key of the Git sync ("CBRX" "SYNC").
const SYNC_LOCK_KEY: i64 = 0x4342_5258_5359_4E43;

/// At most this many issues are stored.
const MAX_STORED_ISSUES: usize = 50;

/// Where the installation stands relative to the Git repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Never,
    InSync,
    OutOfSync,
}

impl SyncStatus {
    fn parse(s: &str) -> Result<Self, StateError> {
        match s {
            "never" => Ok(SyncStatus::Never),
            "in_sync" => Ok(SyncStatus::InSync),
            "out_of_sync" => Ok(SyncStatus::OutOfSync),
            other => Err(StateError::Db(sqlx::Error::Decode(
                format!("unknown sync status {other:?}").into(),
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyncState {
    pub status: SyncStatus,
    pub active_commit: Option<String>,
    pub last_success_at: Option<OffsetDateTime>,
    pub last_attempt_at: Option<OffsetDateTime>,
    pub attempted_commit: Option<String>,
    pub issues: Vec<SyncIssue>,
}

/// Names added, changed and removed by one apply (sorted).
#[derive(Serialize, Deserialize, Default, PartialEq, Debug, Clone)]
pub struct ChangeSet {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Serialize, Deserialize, Default, PartialEq, Debug, Clone)]
pub struct SyncChanges {
    pub hosts: ChangeSet,
    pub templates: ChangeSet,
    pub notifications: ChangeSet,
    pub dashboards: ChangeSet,
    pub displays: ChangeSet,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyncHistoryEntry {
    pub commit: String,
    pub applied_at: OffsetDateTime,
    pub changes: SyncChanges,
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("refusing to deactivate {removed} of {active} active hosts")]
    MassRemoval { removed: usize, active: usize },
    #[error(transparent)]
    State(#[from] StateError),
}

impl From<sqlx::Error> for ApplyError {
    fn from(e: sqlx::Error) -> Self {
        ApplyError::State(e.into())
    }
}

/// The cluster-wide right to run the Git sync; holds one dedicated connection.
///
/// Call [`SyncLock::release`]. If the value is dropped instead, its connection is detached
/// from the pool and closed, which ends the session and so releases the lock.
pub struct SyncLock {
    conn: Option<PoolConnection<Postgres>>,
}

impl SyncLock {
    /// Unlocks and returns the connection to the pool.
    pub async fn release(mut self) -> Result<(), StateError> {
        let Some(mut conn) = self.conn.take() else {
            return Ok(());
        };
        let result = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
            .bind(SYNC_LOCK_KEY)
            .fetch_one(&mut *conn)
            .await;
        match result {
            Ok(true) => Ok(()),
            Ok(false) => {
                // Not held by this session: the connection is in an unexpected state.
                drop(conn.detach());
                Ok(())
            }
            Err(e) => {
                drop(conn.detach());
                Err(e.into())
            }
        }
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            // Closing the session releases the advisory lock; never pool a locked connection.
            drop(conn.detach());
        }
    }
}

impl std::fmt::Debug for SyncLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SyncLock")
    }
}

type HostSnapshot = (
    Uuid,
    String,
    bool,
    Option<String>,
    Vec<String>,
    Option<String>,
    Option<serde_json::Value>,
);

fn kind_set<'a>(changes: &'a mut SyncChanges, kind: &str) -> &'a mut ChangeSet {
    match kind {
        "template" => &mut changes.templates,
        "notification" => &mut changes.notifications,
        "dashboard" => &mut changes.dashboards,
        _ => &mut changes.displays,
    }
}

impl StateStore {
    /// Applies a compiled configuration in one transaction and records it in the history.
    pub async fn apply_config(
        &self,
        commit: &str,
        compiled: &Compiled,
        now: OffsetDateTime,
        max_host_removal_percent: u8,
    ) -> Result<SyncChanges, ApplyError> {
        let mut tx = self.pool.begin().await?;
        let mut changes = SyncChanges::default();

        // Lock all host rows so a concurrent enrolment cannot slip a credential in.
        let existing: Vec<HostSnapshot> = sqlx::query_as(
            "SELECT id, name, active, address, groups, source_file, config \
             FROM hosts ORDER BY name FOR UPDATE",
        )
        .fetch_all(&mut *tx)
        .await?;
        let by_name: BTreeMap<&str, &HostSnapshot> =
            existing.iter().map(|h| (h.1.as_str(), h)).collect();
        let wanted: BTreeSet<&str> = compiled.hosts.iter().map(|h| h.name.as_str()).collect();

        let active = existing.iter().filter(|h| h.2).count();
        let removed: Vec<&HostSnapshot> = existing
            .iter()
            .filter(|h| h.2 && !wanted.contains(h.1.as_str()))
            .collect();
        if active >= 10 && removed.len() * 100 > active * usize::from(max_host_removal_percent) {
            return Err(ApplyError::MassRemoval {
                removed: removed.len(),
                active,
            });
        }

        for host in &compiled.hosts {
            let config = serde_json::to_value(&host.effective)
                .map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
            match by_name.get(host.name.as_str()) {
                None => {
                    sqlx::query(
                        "INSERT INTO hosts (id, name, address, groups, source_file, config) \
                         VALUES ($1, $2, $3, $4, $5, $6)",
                    )
                    .bind(Uuid::new_v4())
                    .bind(&host.name)
                    .bind(&host.address)
                    .bind(&host.groups)
                    .bind(&host.source_file)
                    .bind(Json(&config))
                    .execute(&mut *tx)
                    .await?;
                    changes.hosts.added.push(host.name.clone());
                }
                Some(old) => {
                    let same = old.2
                        && old.3 == host.address
                        && old.4 == host.groups
                        && old.5.as_deref() == Some(host.source_file.as_str())
                        && old.6.as_ref() == Some(&config);
                    if same {
                        continue;
                    }
                    sqlx::query(
                        "UPDATE hosts SET address = $2, groups = $3, source_file = $4, \
                         config = $5, active = true WHERE id = $1",
                    )
                    .bind(old.0)
                    .bind(&host.address)
                    .bind(&host.groups)
                    .bind(&host.source_file)
                    .bind(Json(&config))
                    .execute(&mut *tx)
                    .await?;
                    changes.hosts.changed.push(host.name.clone());
                }
            }
        }

        if !removed.is_empty() {
            let ids: Vec<Uuid> = removed.iter().map(|h| h.0).collect();
            sqlx::query("UPDATE hosts SET active = false, config = NULL WHERE id = ANY($1)")
                .bind(&ids)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM agent_credentials WHERE host_id = ANY($1)")
                .bind(&ids)
                .execute(&mut *tx)
                .await?;
            changes
                .hosts
                .removed
                .extend(removed.iter().map(|h| h.1.clone()));
        }

        // Documents: compare with what is stored, then replace everything.
        let old_docs: Vec<(String, String, String, serde_json::Value)> =
            sqlx::query_as("SELECT kind, id, source_file, body FROM config_documents")
                .fetch_all(&mut *tx)
                .await?;
        let old_docs: BTreeMap<(&str, &str), (&str, &serde_json::Value)> = old_docs
            .iter()
            .map(|d| ((d.0.as_str(), d.1.as_str()), (d.2.as_str(), &d.3)))
            .collect();
        let mut new_keys = BTreeSet::new();
        for doc in &compiled.documents {
            let kind = doc.kind.as_str();
            new_keys.insert((kind, doc.id.as_str()));
            match old_docs.get(&(kind, doc.id.as_str())) {
                None => kind_set(&mut changes, kind).added.push(doc.id.clone()),
                Some((file, body)) if *file != doc.source_file || **body != doc.body => {
                    kind_set(&mut changes, kind).changed.push(doc.id.clone());
                }
                Some(_) => {}
            }
        }
        for (kind, id) in old_docs.keys() {
            if !new_keys.contains(&(*kind, *id)) {
                kind_set(&mut changes, kind).removed.push((*id).to_owned());
            }
        }
        sqlx::query("DELETE FROM config_documents")
            .execute(&mut *tx)
            .await?;
        for doc in &compiled.documents {
            sqlx::query(
                "INSERT INTO config_documents (kind, id, source_file, body) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(doc.kind.as_str())
            .bind(&doc.id)
            .bind(&doc.source_file)
            .bind(Json(&doc.body))
            .execute(&mut *tx)
            .await?;
        }

        for set in [
            &mut changes.hosts,
            &mut changes.templates,
            &mut changes.notifications,
            &mut changes.dashboards,
            &mut changes.displays,
        ] {
            set.added.sort();
            set.changed.sort();
            set.removed.sort();
        }

        sqlx::query(
            "UPDATE sync_state SET status = 'in_sync', active_commit = $1, \
             last_success_at = $2, last_attempt_at = $2, attempted_commit = $1, issues = '[]'",
        )
        .bind(commit)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO sync_history (commit, applied_at, changes) VALUES ($1, $2, $3)")
            .bind(commit)
            .bind(now)
            .bind(Json(&changes))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(changes)
    }

    /// Marks the installation Out Of Sync; the active configuration stays untouched.
    pub async fn record_sync_failure(
        &self,
        attempted_commit: Option<&str>,
        issues: &[SyncIssue],
        now: OffsetDateTime,
    ) -> Result<(), StateError> {
        let issues = &issues[..issues.len().min(MAX_STORED_ISSUES)];
        sqlx::query(
            "UPDATE sync_state SET status = 'out_of_sync', last_attempt_at = $1, \
             attempted_commit = $2, issues = $3",
        )
        .bind(now)
        .bind(attempted_commit)
        .bind(Json(issues))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The repository has not changed since the last successful sync.
    pub async fn record_sync_unchanged(&self, now: OffsetDateTime) -> Result<(), StateError> {
        sqlx::query(
            "UPDATE sync_state SET status = 'in_sync', last_success_at = $1, \
             last_attempt_at = $1, issues = '[]'",
        )
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn sync_state(&self) -> Result<SyncState, StateError> {
        let row = sqlx::query(
            "SELECT status, active_commit, last_success_at, last_attempt_at, \
             attempted_commit, issues FROM sync_state",
        )
        .fetch_one(&self.pool)
        .await?;
        let status: String = row.try_get("status")?;
        let issues: Json<Vec<SyncIssue>> = row.try_get("issues")?;
        Ok(SyncState {
            status: SyncStatus::parse(&status)?,
            active_commit: row.try_get("active_commit")?,
            last_success_at: row.try_get("last_success_at")?,
            last_attempt_at: row.try_get("last_attempt_at")?,
            attempted_commit: row.try_get("attempted_commit")?,
            issues: issues.0,
        })
    }

    /// The most recent applied revisions, newest first.
    pub async fn sync_history(&self, limit: i64) -> Result<Vec<SyncHistoryEntry>, StateError> {
        let rows: Vec<(String, OffsetDateTime, Json<SyncChanges>)> = sqlx::query_as(
            "SELECT commit, applied_at, changes FROM sync_history ORDER BY id DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(commit, applied_at, changes)| SyncHistoryEntry {
                commit,
                applied_at,
                changes: changes.0,
            })
            .collect())
    }

    /// The effective configuration of an active host synced from Git.
    pub async fn host_config(&self, host_id: Uuid) -> Result<Option<EffectiveHost>, StateError> {
        let row: Option<Option<Json<EffectiveHost>>> =
            sqlx::query_scalar("SELECT config FROM hosts WHERE id = $1 AND active")
                .bind(host_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.flatten().map(|j| j.0))
    }

    /// Takes the sync lock if no other instance holds it.
    pub async fn try_sync_lock(&self) -> Result<Option<SyncLock>, StateError> {
        let mut conn = self.pool.acquire().await?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(SYNC_LOCK_KEY)
            .fetch_one(&mut *conn)
            .await?;
        Ok(locked.then_some(SyncLock { conn: Some(conn) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitops::{Compiled, DocumentKind, SyncIssue, compile, read_tree};
    use sqlx::PgPool;
    use time::{Duration, OffsetDateTime};

    use crate::state::StateStore;

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn example() -> Compiled {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        compile(&read_tree(&dir, "c1").unwrap()).unwrap()
    }

    fn issue(reason: &str) -> SyncIssue {
        SyncIssue {
            file: Some("hosts/x.json".into()),
            line: Some(3),
            column: Some(4),
            reason: reason.into(),
        }
    }

    fn host(name: &str) -> crate::gitops::CompiledHost {
        let mut h = example().hosts[0].clone();
        h.name = name.to_owned();
        h.source_file = format!("hosts/{name}.json");
        h
    }

    async fn active_count(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM hosts WHERE active")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn history_len(store: &StateStore) -> usize {
        store.sync_history(100).await.unwrap().len()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn first_apply_creates_everything(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        assert_eq!(store.sync_state().await.unwrap().status, SyncStatus::Never);
        let compiled = example();
        let changes = store.apply_config("c1", &compiled, t0(), 50).await.unwrap();
        assert_eq!(changes.hosts.added, ["db-01", "web-01"]);
        assert!(changes.hosts.changed.is_empty() && changes.hosts.removed.is_empty());
        assert_eq!(changes.templates.added, ["linux-base"]);
        assert_eq!(changes.dashboards.added, ["production"]);
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.status, SyncStatus::InSync);
        assert_eq!(state.active_commit.as_deref(), Some("c1"));
        assert_eq!(state.attempted_commit.as_deref(), Some("c1"));
        assert_eq!(state.last_success_at, Some(t0()));
        assert_eq!(state.last_attempt_at, Some(t0()));
        assert!(state.issues.is_empty());
        let history = store.sync_history(10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].commit, "c1");
        assert_eq!(history[0].applied_at, t0());
        assert_eq!(history[0].changes.hosts.added, ["db-01", "web-01"]);
        let web = store.host_by_name("web-01").await.unwrap().unwrap();
        let expected = compiled.hosts.iter().find(|h| h.name == "web-01").unwrap();
        assert_eq!(
            store.host_config(web.id).await.unwrap(),
            Some(expected.effective.clone())
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn second_apply_reports_changes(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let mut compiled = example();
        store
            .apply_config("c1", &compiled, t0(), 100)
            .await
            .unwrap();
        let web = compiled
            .hosts
            .iter_mut()
            .find(|h| h.name == "web-01")
            .unwrap();
        web.effective.items[0].interval_secs = 30;
        compiled
            .documents
            .retain(|d| d.kind != DocumentKind::Dashboard);
        let changes = store
            .apply_config("c2", &compiled, t0() + Duration::minutes(1), 100)
            .await
            .unwrap();
        assert_eq!(changes.hosts.changed, ["web-01"]);
        assert!(changes.hosts.added.is_empty() && changes.hosts.removed.is_empty());
        assert_eq!(changes.dashboards.removed, ["production"]);
        assert!(changes.dashboards.added.is_empty());
        assert_eq!(changes.templates, ChangeSet::default());
        assert_eq!(history_len(&store).await, 2);
        // Same content, new commit: a history row with empty changes.
        let again = store
            .apply_config("c3", &compiled, t0(), 100)
            .await
            .unwrap();
        assert_eq!(again, SyncChanges::default());
        let history = store.sync_history(10).await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].commit, "c3");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn removed_host_is_deactivated_and_credential_revoked(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let mut compiled = example();
        store
            .apply_config("c1", &compiled, t0(), 100)
            .await
            .unwrap();
        let db = store.host_by_name("db-01").await.unwrap().unwrap();
        let cred = store.bind_credential(db.id).await.unwrap();
        compiled.hosts.retain(|h| h.name != "db-01");
        let changes = store
            .apply_config("c2", &compiled, t0(), 100)
            .await
            .unwrap();
        assert_eq!(changes.hosts.removed, ["db-01"]);
        assert!(!store.host_by_name("db-01").await.unwrap().unwrap().active);
        assert!(store.host_for_credential(&cred).await.unwrap().is_none());
        assert!(store.host_config(db.id).await.unwrap().is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn readded_host_is_active_without_credential(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let full = example();
        let mut partial = full.clone();
        store.apply_config("c1", &full, t0(), 100).await.unwrap();
        let db = store.host_by_name("db-01").await.unwrap().unwrap();
        let cred = store.bind_credential(db.id).await.unwrap();
        partial.hosts.retain(|h| h.name != "db-01");
        store.apply_config("c2", &partial, t0(), 100).await.unwrap();
        let changes = store.apply_config("c3", &full, t0(), 100).await.unwrap();
        assert_eq!(changes.hosts.changed, ["db-01"]);
        assert!(changes.hosts.added.is_empty());
        let db_again = store.host_by_name("db-01").await.unwrap().unwrap();
        assert!(db_again.active);
        assert_eq!(db_again.id, db.id);
        assert!(store.host_for_credential(&cred).await.unwrap().is_none());
        store.bind_credential(db.id).await.unwrap();
        assert!(store.host_config(db.id).await.unwrap().is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn failure_keeps_active_configuration(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        store
            .apply_config("c1", &example(), t0(), 100)
            .await
            .unwrap();
        let t1 = t0() + Duration::minutes(5);
        store
            .record_sync_failure(Some("bad"), &[issue("broken")], t1)
            .await
            .unwrap();
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.status, SyncStatus::OutOfSync);
        assert_eq!(state.active_commit.as_deref(), Some("c1"));
        assert_eq!(state.attempted_commit.as_deref(), Some("bad"));
        assert_eq!(state.last_success_at, Some(t0()));
        assert_eq!(state.last_attempt_at, Some(t1));
        assert_eq!(state.issues, vec![issue("broken")]);
        assert_eq!(active_count(&pool).await, 2);
        assert_eq!(history_len(&store).await, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn failure_stores_at_most_fifty_issues(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let issues: Vec<_> = (0..80).map(|i| issue(&format!("r{i}"))).collect();
        store
            .record_sync_failure(None, &issues, t0())
            .await
            .unwrap();
        assert_eq!(store.sync_state().await.unwrap().issues.len(), 50);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unchanged_clears_out_of_sync(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        store
            .apply_config("c1", &example(), t0(), 100)
            .await
            .unwrap();
        store
            .record_sync_failure(Some("bad"), &[issue("broken")], t0())
            .await
            .unwrap();
        let t1 = t0() + Duration::minutes(10);
        store.record_sync_unchanged(t1).await.unwrap();
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.status, SyncStatus::InSync);
        assert!(state.issues.is_empty());
        assert_eq!(state.last_success_at, Some(t1));
        assert_eq!(state.last_attempt_at, Some(t1));
        assert_eq!(history_len(&store).await, 1);
    }

    async fn ten_hosts(store: &StateStore) -> Compiled {
        let compiled = Compiled {
            hosts: (0..10).map(|i| host(&format!("h{i}"))).collect(),
            documents: vec![],
        };
        store
            .apply_config("c1", &compiled, t0(), 100)
            .await
            .unwrap();
        compiled
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn mass_removal_is_refused(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let mut compiled = ten_hosts(&store).await;
        for i in 0..10 {
            let h = store.host_by_name(&format!("h{i}")).await.unwrap().unwrap();
            store.bind_credential(h.id).await.unwrap();
        }
        compiled.hosts.truncate(4);
        let err = store
            .apply_config("c2", &compiled, t0(), 50)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ApplyError::MassRemoval {
                removed: 6,
                active: 10
            }
        ));
        assert_eq!(active_count(&pool).await, 10);
        let creds: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_credentials")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(creds, 10);
        assert_eq!(history_len(&store).await, 1);
        store
            .apply_config("c2", &compiled, t0(), 100)
            .await
            .unwrap();
        assert_eq!(active_count(&pool).await, 4);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn mass_removal_guard_needs_ten_active_hosts(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let compiled = Compiled {
            hosts: (0..9).map(|i| host(&format!("h{i}"))).collect(),
            documents: vec![],
        };
        store
            .apply_config("c1", &compiled, t0(), 100)
            .await
            .unwrap();
        let few = Compiled {
            hosts: compiled.hosts[..2].to_vec(),
            documents: vec![],
        };
        store.apply_config("c2", &few, t0(), 10).await.unwrap();
        assert_eq!(active_count(&pool).await, 2);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn apply_is_atomic(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        store
            .apply_config("c1", &example(), t0(), 100)
            .await
            .unwrap();
        let mut broken = example();
        broken.hosts.push(host("new-host"));
        let dup = broken.documents[0].clone();
        broken.documents.push(dup);
        let err = store
            .apply_config("c2", &broken, t0(), 100)
            .await
            .unwrap_err();
        assert!(matches!(err, ApplyError::State(_)));
        assert!(store.host_by_name("new-host").await.unwrap().is_none());
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.active_commit.as_deref(), Some("c1"));
        assert_eq!(history_len(&store).await, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn sync_lock_is_exclusive(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let first = store.try_sync_lock().await.unwrap().expect("first lock");
        assert!(store.try_sync_lock().await.unwrap().is_none());
        first.release().await.unwrap();
        let second = store.try_sync_lock().await.unwrap().expect("after release");
        second.release().await.unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn dropped_lock_does_not_stay_held(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let lock = store.try_sync_lock().await.unwrap().expect("first lock");
        drop(lock);
        let mut got = None;
        for _ in 0..50 {
            if let Some(l) = store.try_sync_lock().await.unwrap() {
                got = Some(l);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        got.expect("lock must be free after drop")
            .release()
            .await
            .unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn history_is_newest_first_and_limited(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let compiled = example();
        for i in 1..=3 {
            store
                .apply_config(&format!("c{i}"), &compiled, t0(), 100)
                .await
                .unwrap();
        }
        let two = store.sync_history(2).await.unwrap();
        let commits: Vec<_> = two.iter().map(|e| e.commit.as_str()).collect();
        assert_eq!(commits, ["c3", "c2"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn manual_hosts_are_removed_by_first_sync(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let manual = store.create_host("manual").await.unwrap();
        let cred = store.bind_credential(manual.id).await.unwrap();
        let changes = store
            .apply_config("c1", &example(), t0(), 100)
            .await
            .unwrap();
        assert_eq!(changes.hosts.removed, ["manual"]);
        assert!(!store.host_by_name("manual").await.unwrap().unwrap().active);
        assert!(store.host_for_credential(&cred).await.unwrap().is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unchanged_active_host_is_not_reported(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        store
            .apply_config("c1", &example(), t0(), 100)
            .await
            .unwrap();
        let changes = store
            .apply_config("c2", &example(), t0(), 100)
            .await
            .unwrap();
        assert_eq!(changes, SyncChanges::default());
    }
}
