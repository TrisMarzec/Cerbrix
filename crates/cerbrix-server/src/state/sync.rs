//! Applying a compiled configuration, sync state, history and the sync lock.

use std::collections::{BTreeMap, BTreeSet};

use cerbrix_common::{Selector, TriggerExpr, parse_trigger};
use serde::{Deserialize, Serialize};
use sqlx::pool::PoolConnection;
use sqlx::types::Json;
use sqlx::{Postgres, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::alerting::{close_open_problems, delete_trigger_states, delete_unwatched_series};
use super::{StateError, StateStore};
use crate::alerting::nodata::item_nodata_selectors;
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

/// The result of applying a revision.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub changes: SyncChanges,
    /// Open problems closed because their host or trigger is no longer monitored.
    pub problems_closed: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyncHistoryEntry {
    pub commit: String,
    pub applied_at: OffsetDateTime,
    pub changes: SyncChanges,
}

/// A host as stored, with what Git configured for it.
#[derive(Debug, Clone, PartialEq)]
pub struct HostDetails {
    pub name: String,
    pub active: bool,
    pub address: Option<String>,
    pub groups: Vec<String>,
    pub source_file: Option<String>,
    pub config: Option<EffectiveHost>,
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

/// What a trigger watches. Two triggers with the same name and the same `Watches` are the
/// same trigger as far as its stored state goes; threshold, operator, hold time, window and
/// severity are not part of it.
#[derive(PartialEq)]
enum Watches {
    Comparison(Selector),
    NoData(Option<Selector>),
}

fn watches(expression: &str) -> Option<Watches> {
    match parse_trigger(expression).ok()? {
        TriggerExpr::Compare { selector, .. } => Some(Watches::Comparison(selector)),
        TriggerExpr::NoData { selector, .. } => Some(Watches::NoData(selector)),
    }
}

/// Closes what the new revision no longer monitors on hosts that stay: open problems and
/// stored states of triggers that are gone, and of triggers that now watch something else
/// (another kind or selector), because no evaluation would visit their old instances again.
/// A trigger that only changed its threshold, operator, hold time, window or severity keeps
/// both. A stored expression that no longer parses counts as changed. A stored configuration
/// that cannot be read at all is logged, and then only the triggers whose name is gone are
/// closed. Also deletes the arrivals in `series_seen` that no item-level no-data trigger of the
/// new revision watches, so that a trigger that is removed and later restored starts from the
/// first value that arrives after the restore instead of from a stale arrival. Returns the
/// number of problems closed.
async fn close_unmonitored(
    tx: &mut sqlx::PgConnection,
    stored: &BTreeMap<&str, &HostSnapshot>,
    compiled: &Compiled,
    now: OffsetDateTime,
    queue_mail: bool,
) -> Result<usize, StateError> {
    let mut host_ids = Vec::new();
    let mut keep = Vec::new();
    let mut watched = Vec::new();
    for host in &compiled.hosts {
        let Some(old) = stored.get(host.name.as_str()).filter(|h| h.2) else {
            continue;
        };
        host_ids.push(old.0);
        watched.extend(
            item_nodata_selectors(&host.effective)
                .into_iter()
                .map(|s| (old.0, s)),
        );
        // `None`: a stored configuration that cannot be read. A host without configuration
        // had no triggers.
        let old_triggers: Option<BTreeMap<String, String>> = match &old.6 {
            None => Some(BTreeMap::new()),
            Some(value) => match serde_json::from_value::<EffectiveHost>(value.clone()) {
                Ok(h) => Some(
                    h.triggers
                        .into_iter()
                        .map(|t| (t.name, t.expression))
                        .collect(),
                ),
                Err(err) => {
                    tracing::warn!(
                        host = %host.name,
                        %err,
                        "the stored configuration of the host cannot be read; problems of \
                         triggers that still exist by name are kept"
                    );
                    None
                }
            },
        };
        for trigger in &host.effective.triggers {
            // A trigger that is new by name has nothing to close; one whose old definition
            // cannot be read is kept by its name.
            let unchanged = match old_triggers.as_ref().and_then(|t| t.get(&trigger.name)) {
                None => true,
                Some(old_expr) => {
                    let new = watches(&trigger.expression);
                    new.is_some() && watches(old_expr) == new
                }
            };
            if unchanged {
                keep.push((old.0, trigger.name.clone()));
            }
        }
    }
    if host_ids.is_empty() {
        return Ok(0);
    }
    let closed =
        close_open_problems(tx, &host_ids, &keep, "trigger_removed", now, queue_mail).await?;
    delete_trigger_states(tx, &host_ids, &keep).await?;
    delete_unwatched_series(tx, &host_ids, &watched).await?;
    Ok(closed)
}

impl StateStore {
    /// Applies a compiled configuration in one transaction and records it in the history.
    ///
    /// Problems of removed hosts and of removed or re-targeted triggers are closed in the
    /// same transaction; with `queue_mail` the people mailed about
    /// their start are told they ended.
    pub async fn apply_config(
        &self,
        commit: &str,
        compiled: &Compiled,
        now: OffsetDateTime,
        max_host_removal_percent: u8,
        queue_mail: bool,
    ) -> Result<Applied, ApplyError> {
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

        let mut problems_closed = 0;
        if !removed.is_empty() {
            let ids: Vec<Uuid> = removed.iter().map(|h| h.0).collect();
            problems_closed +=
                close_open_problems(&mut tx, &ids, &[], "host_removed", now, queue_mail).await?;
            delete_trigger_states(&mut tx, &ids, &[]).await?;
            sqlx::query("DELETE FROM series_seen WHERE host_id = ANY($1)")
                .bind(&ids)
                .execute(&mut *tx)
                .await?;
            // A host that returns later starts clean: A6 (no data) applies to it again.
            sqlx::query(
                "UPDATE hosts SET active = false, config = NULL, last_seen_at = NULL \
                 WHERE id = ANY($1)",
            )
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
        problems_closed += close_unmonitored(&mut tx, &by_name, compiled, now, queue_mail).await?;

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
        Ok(Applied {
            changes,
            problems_closed,
        })
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

    /// A host by name with its Git-derived settings, active or not.
    pub async fn host_details(&self, name: &str) -> Result<Option<HostDetails>, StateError> {
        type Row = (
            String,
            bool,
            Option<String>,
            Vec<String>,
            Option<String>,
            Option<Json<EffectiveHost>>,
        );
        let row: Option<Row> = sqlx::query_as(
            "SELECT name, active, address, groups, source_file, config \
             FROM hosts WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(name, active, address, groups, source_file, config)| HostDetails {
                name,
                active,
                address,
                groups,
                source_file,
                config: config.map(|j| j.0),
            },
        ))
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
        let changes = store
            .apply_config("c1", &compiled, t0(), 50, false)
            .await
            .unwrap()
            .changes;
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
            .apply_config("c1", &compiled, t0(), 100, false)
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
            .apply_config("c2", &compiled, t0() + Duration::minutes(1), 100, false)
            .await
            .unwrap()
            .changes;
        assert_eq!(changes.hosts.changed, ["web-01"]);
        assert!(changes.hosts.added.is_empty() && changes.hosts.removed.is_empty());
        assert_eq!(changes.dashboards.removed, ["production"]);
        assert!(changes.dashboards.added.is_empty());
        assert_eq!(changes.templates, ChangeSet::default());
        assert_eq!(history_len(&store).await, 2);
        // Same content, new commit: a history row with empty changes.
        let again = store
            .apply_config("c3", &compiled, t0(), 100, false)
            .await
            .unwrap()
            .changes;
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
            .apply_config("c1", &compiled, t0(), 100, false)
            .await
            .unwrap();
        let db = store.host_by_name("db-01").await.unwrap().unwrap();
        let cred = store.bind_credential(db.id).await.unwrap();
        compiled.hosts.retain(|h| h.name != "db-01");
        let changes = store
            .apply_config("c2", &compiled, t0(), 100, false)
            .await
            .unwrap()
            .changes;
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
        store
            .apply_config("c1", &full, t0(), 100, false)
            .await
            .unwrap();
        let db = store.host_by_name("db-01").await.unwrap().unwrap();
        let cred = store.bind_credential(db.id).await.unwrap();
        partial.hosts.retain(|h| h.name != "db-01");
        store
            .apply_config("c2", &partial, t0(), 100, false)
            .await
            .unwrap();
        let changes = store
            .apply_config("c3", &full, t0(), 100, false)
            .await
            .unwrap()
            .changes;
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
            .apply_config("c1", &example(), t0(), 100, false)
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
            .apply_config("c1", &example(), t0(), 100, false)
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
            .apply_config("c1", &compiled, t0(), 100, false)
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
            .apply_config("c2", &compiled, t0(), 50, false)
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
            .apply_config("c2", &compiled, t0(), 100, false)
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
            .apply_config("c1", &compiled, t0(), 100, false)
            .await
            .unwrap();
        let few = Compiled {
            hosts: compiled.hosts[..2].to_vec(),
            documents: vec![],
        };
        store
            .apply_config("c2", &few, t0(), 10, false)
            .await
            .unwrap();
        assert_eq!(active_count(&pool).await, 2);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn apply_is_atomic(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        store
            .apply_config("c1", &example(), t0(), 100, false)
            .await
            .unwrap();
        let mut broken = example();
        broken.hosts.push(host("new-host"));
        let dup = broken.documents[0].clone();
        broken.documents.push(dup);
        let err = store
            .apply_config("c2", &broken, t0(), 100, false)
            .await
            .unwrap_err();
        assert!(matches!(err, ApplyError::State(_)));
        assert!(store.host_by_name("new-host").await.unwrap().is_none());
        let state = store.sync_state().await.unwrap();
        assert_eq!(state.active_commit.as_deref(), Some("c1"));
        assert_eq!(history_len(&store).await, 1);
    }

    /// Number of sessions in this database holding the sync advisory lock (from `pg_locks`).
    async fn lock_holders(pool: &PgPool) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND granted \
             AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
             AND classid::bigint = ($1::bigint >> 32) \
             AND objid::bigint = ($1::bigint & 4294967295)",
        )
        .bind(SYNC_LOCK_KEY)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn wait_until_unlocked(pool: &PgPool) {
        for _ in 0..50 {
            if lock_holders(pool).await == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("sync lock is still held");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn sync_lock_is_exclusive(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let first = store.try_sync_lock().await.unwrap().expect("first lock");
        assert_eq!(lock_holders(&pool).await, 1);
        assert!(store.try_sync_lock().await.unwrap().is_none());
        first.release().await.unwrap();
        // Checked from pg_locks: the unlock really happened, not just a pooled connection.
        assert_eq!(lock_holders(&pool).await, 0);
        let second = store.try_sync_lock().await.unwrap().expect("after release");
        second.release().await.unwrap();
        assert_eq!(lock_holders(&pool).await, 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn dropped_lock_does_not_stay_held(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let lock = store.try_sync_lock().await.unwrap().expect("first lock");
        assert_eq!(lock_holders(&pool).await, 1);
        drop(lock);
        wait_until_unlocked(&pool).await;
        let again = store
            .try_sync_lock()
            .await
            .unwrap()
            .expect("free after drop");
        again.release().await.unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn history_is_newest_first_and_limited(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let compiled = example();
        for i in 1..=3 {
            store
                .apply_config(&format!("c{i}"), &compiled, t0(), 100, false)
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
            .apply_config("c1", &example(), t0(), 100, false)
            .await
            .unwrap()
            .changes;
        assert_eq!(changes.hosts.removed, ["manual"]);
        assert!(!store.host_by_name("manual").await.unwrap().unwrap().active);
        assert!(store.host_for_credential(&cred).await.unwrap().is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unchanged_active_host_is_not_reported(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        store
            .apply_config("c1", &example(), t0(), 100, false)
            .await
            .unwrap();
        let changes = store
            .apply_config("c2", &example(), t0(), 100, false)
            .await
            .unwrap()
            .changes;
        assert_eq!(changes, SyncChanges::default());
    }

    // ---- closing what is no longer monitored ----

    use crate::alerting::{Params, active_triggers, eval::evaluate};
    use crate::gitops::{CompiledHost, EffectiveTrigger};
    use cerbrix_common::MetricValue;
    use cerbrix_common::config::Severity;

    const T0_MS: i64 = 1_800_000_000_000;

    fn later() -> OffsetDateTime {
        t0() + Duration::minutes(10)
    }

    fn host_with(name: &str, triggers: &[(&str, &str)]) -> CompiledHost {
        let mut h = host(name);
        h.groups = vec!["production".to_owned()];
        h.effective.triggers = triggers
            .iter()
            .map(|(n, e)| EffectiveTrigger {
                name: (*n).to_owned(),
                expression: (*e).to_owned(),
                severity: Severity::High,
                origin: "host".to_owned(),
            })
            .collect();
        h
    }

    /// The hosts plus the example notification rule (`ops@example.com`, high, production).
    fn world(hosts: Vec<CompiledHost>) -> Compiled {
        Compiled {
            hosts,
            documents: example().documents,
        }
    }

    async fn id_of(store: &StateStore, name: &str) -> Uuid {
        store.host_by_name(name).await.unwrap().unwrap().id
    }

    /// Delivers one value through the real evaluation; returns the number of events.
    async fn feed(
        store: &StateStore,
        host: Uuid,
        key: &str,
        params: &[(&str, &str)],
        value: f64,
        at_ms: i64,
    ) -> usize {
        let v = MetricValue {
            key: key.to_owned(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            ts_ms: at_ms,
            value,
        };
        let events = store
            .evaluate_host(host, true, |ctx| {
                let triggers = ctx.config.as_ref().map(active_triggers).unwrap_or_default();
                evaluate(&triggers, &ctx.states, &[v], at_ms, i64::MAX / 2)
            })
            .await
            .unwrap();
        events.len()
    }

    /// (host, trigger, resolved_at, reason) of every problem, ordered.
    type ProblemRow = (String, String, Option<OffsetDateTime>, Option<String>);

    async fn problems(pool: &PgPool) -> Vec<ProblemRow> {
        sqlx::query_as(
            "SELECT h.name, p.trigger_name, p.resolved_at, p.resolve_reason \
             FROM problems p JOIN hosts h ON h.id = p.host_id \
             ORDER BY h.name, p.trigger_name, p.started_at",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn count(pool: &PgPool, table: &str) -> i64 {
        let sql = match table {
            "trigger_states" => "SELECT count(*) FROM trigger_states",
            "series_seen" => "SELECT count(*) FROM series_seen",
            "notification_outbox" => "SELECT count(*) FROM notification_outbox",
            other => panic!("unexpected table {other}"),
        };
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }

    async fn resolved_mail(pool: &PgPool) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT recipient FROM notification_outbox WHERE event = 'resolved' ORDER BY 1",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn last_seen(pool: &PgPool, host: Uuid) -> Option<OffsetDateTime> {
        sqlx::query_scalar("SELECT last_seen_at FROM hosts WHERE id = $1")
            .bind(host)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn removed_host_closes_its_problems(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let cpu = [
            ("CPU", "cpu.usage > 90"),
            ("CPU silent", "cpu.usage no data for 5m"),
        ];
        let both = world(vec![host_with("db-01", &cpu), host_with("web-01", &cpu)]);
        store
            .apply_config("c1", &both, t0(), 100, true)
            .await
            .unwrap();
        for name in ["db-01", "web-01"] {
            let id = id_of(&store, name).await;
            assert_eq!(feed(&store, id, "cpu.usage", &[], 95.0, T0_MS).await, 1);
            store
                .touch_seen(id, t0(), &[("cpu.usage".to_owned(), Params::new())])
                .await
                .unwrap();
        }
        let db = id_of(&store, "db-01").await;
        assert_eq!(count(&pool, "series_seen").await, 2);

        let only_web = world(vec![host_with("web-01", &cpu)]);
        let applied = store
            .apply_config("c2", &only_web, later(), 100, true)
            .await
            .unwrap();
        assert_eq!(applied.problems_closed, 1);
        assert_eq!(applied.changes.hosts.removed, ["db-01"]);

        let rows = problems(&pool).await;
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            (
                "db-01".into(),
                "CPU".into(),
                Some(later()),
                Some("host_removed".into())
            )
        );
        assert_eq!((rows[1].2, rows[1].3.as_deref()), (None, None));
        assert_eq!(resolved_mail(&pool).await, ["ops@example.com"]);
        for sql in [
            "SELECT count(*) FROM trigger_states WHERE host_id = $1",
            "SELECT count(*) FROM series_seen WHERE host_id = $1",
        ] {
            let left: i64 = sqlx::query_scalar(sql)
                .bind(db)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(left, 0, "{sql}");
        }
        // The other host keeps its rows.
        assert_eq!(count(&pool, "trigger_states").await, 1);
        assert_eq!(count(&pool, "series_seen").await, 1);
        assert_eq!(last_seen(&pool, db).await, None);
        assert!(
            last_seen(&pool, id_of(&store, "web-01").await)
                .await
                .is_some()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn removed_trigger_closes_its_problem(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let both = [
            ("CPU", "cpu.usage > 90"),
            ("Memory", "memory.used_percent > 90"),
        ];
        store
            .apply_config(
                "c1",
                &world(vec![host_with("web-01", &both)]),
                t0(),
                100,
                true,
            )
            .await
            .unwrap();
        let id = id_of(&store, "web-01").await;
        feed(&store, id, "cpu.usage", &[], 95.0, T0_MS).await;
        feed(&store, id, "memory.used_percent", &[], 95.0, T0_MS).await;

        let without_cpu = world(vec![host_with("web-01", &both[1..])]);
        let applied = store
            .apply_config("c2", &without_cpu, later(), 100, true)
            .await
            .unwrap();
        assert_eq!(applied.problems_closed, 1);
        let rows = problems(&pool).await;
        assert_eq!(
            rows[0],
            (
                "web-01".into(),
                "CPU".into(),
                Some(later()),
                Some("trigger_removed".into())
            )
        );
        assert_eq!((rows[1].1.as_str(), rows[1].2), ("Memory", None));
        let states: Vec<String> =
            sqlx::query_scalar("SELECT trigger_name FROM trigger_states WHERE host_id = $1")
                .bind(id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(states, ["Memory"]);
        assert_eq!(resolved_mail(&pool).await, ["ops@example.com"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn resolved_at_is_never_before_started_at(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let cpu = [("CPU", "cpu.usage > 90")];
        store
            .apply_config(
                "c1",
                &world(vec![host_with("web-01", &cpu)]),
                t0(),
                100,
                false,
            )
            .await
            .unwrap();
        let id = id_of(&store, "web-01").await;
        // The value was measured after the time of the later revision.
        let started_ms = T0_MS + 3_600_000;
        feed(&store, id, "cpu.usage", &[], 95.0, started_ms).await;
        store
            .apply_config(
                "c2",
                &world(vec![host_with("web-01", &[])]),
                later(),
                100,
                false,
            )
            .await
            .unwrap();
        let rows = problems(&pool).await;
        assert_eq!(
            rows[0].2,
            Some(OffsetDateTime::from_unix_timestamp(started_ms / 1000).unwrap())
        );
    }

    type StateRow = (bool, Option<OffsetDateTime>, OffsetDateTime, Option<f64>);

    #[sqlx::test(migrations = "./migrations")]
    async fn changed_threshold_keeps_problem_and_state(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let old = world(vec![host_with("web-01", &[("CPU", "cpu.usage > 90")])]);
        store
            .apply_config("c1", &old, t0(), 100, true)
            .await
            .unwrap();
        let id = id_of(&store, "web-01").await;
        feed(&store, id, "cpu.usage", &[], 95.0, T0_MS).await;
        let state_sql = "SELECT firing, condition_since, last_ts, last_value FROM trigger_states";
        let before: Vec<StateRow> = sqlx::query_as(state_sql).fetch_all(&pool).await.unwrap();
        assert_eq!(before.len(), 1);

        let new = world(vec![host_with(
            "web-01",
            &[("CPU", "cpu.usage > 95 for 5m")],
        )]);
        let applied = store
            .apply_config("c2", &new, later(), 100, true)
            .await
            .unwrap();
        assert_eq!(applied.problems_closed, 0);
        assert_eq!(applied.changes.hosts.changed, ["web-01"]);
        let rows = problems(&pool).await;
        assert_eq!((rows.len(), rows[0].2), (1, None));
        let after: Vec<StateRow> = sqlx::query_as(state_sql).fetch_all(&pool).await.unwrap();
        assert_eq!(before, after);

        // Meets the new condition: no second problem, no event.
        let meets = feed(&store, id, "cpu.usage", &[], 99.0, T0_MS + 60_000).await;
        assert_eq!(meets, 0);
        assert_eq!(problems(&pool).await.len(), 1);
        // Does not meet it: resolved because the condition ended.
        let ends = feed(&store, id, "cpu.usage", &[], 10.0, T0_MS + 120_000).await;
        assert_eq!(ends, 1);
        let rows = problems(&pool).await;
        assert_eq!(rows[0].3.as_deref(), Some("condition_ended"));
        assert_eq!(resolved_mail(&pool).await, ["ops@example.com"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn changed_selector_closes_problem(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let old = [
            ("Disk", "disk.used_percent{mount=/} > 90"),
            ("CPU", "cpu.usage > 90"),
            ("Memory", "memory.used_percent > 90"),
        ];
        store
            .apply_config(
                "c1",
                &world(vec![host_with("web-01", &old)]),
                t0(),
                100,
                true,
            )
            .await
            .unwrap();
        let id = id_of(&store, "web-01").await;
        feed(
            &store,
            id,
            "disk.used_percent",
            &[("mount", "/")],
            95.0,
            T0_MS,
        )
        .await;
        feed(&store, id, "cpu.usage", &[], 95.0, T0_MS).await;
        feed(&store, id, "memory.used_percent", &[], 95.0, T0_MS).await;

        let new = [
            ("Disk", "disk.used_percent{mount=/data} > 90"),
            ("CPU", "no data for 5m"),
            ("Memory", "memory.used_percent > 95"),
        ];
        let applied = store
            .apply_config(
                "c2",
                &world(vec![host_with("web-01", &new)]),
                later(),
                100,
                true,
            )
            .await
            .unwrap();
        assert_eq!(applied.problems_closed, 2);
        let rows = problems(&pool).await;
        let by_name: BTreeMap<&str, (Option<OffsetDateTime>, Option<&str>)> = rows
            .iter()
            .map(|r| (r.1.as_str(), (r.2, r.3.as_deref())))
            .collect();
        assert_eq!(by_name["CPU"], (Some(later()), Some("trigger_removed")));
        assert_eq!(by_name["Disk"], (Some(later()), Some("trigger_removed")));
        assert_eq!(by_name["Memory"], (None, None));
        let states: Vec<String> =
            sqlx::query_scalar("SELECT trigger_name FROM trigger_states ORDER BY 1")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(states, ["Memory"]);
        assert_eq!(
            resolved_mail(&pool).await,
            ["ops@example.com", "ops@example.com"]
        );
        assert_eq!(count(&pool, "notification_outbox").await, 5);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unparsable_stored_expression_counts_as_changed(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let cpu = world(vec![host_with("web-01", &[("CPU", "cpu.usage > 90")])]);
        store
            .apply_config("c1", &cpu, t0(), 100, false)
            .await
            .unwrap();
        let id = id_of(&store, "web-01").await;
        feed(&store, id, "cpu.usage", &[], 95.0, T0_MS).await;
        sqlx::query(
            "UPDATE hosts SET config = jsonb_set(config, '{triggers,0,expression}', '\"???\"')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let applied = store
            .apply_config("c2", &cpu, later(), 100, false)
            .await
            .unwrap();
        assert_eq!(applied.problems_closed, 1);
        assert_eq!(
            problems(&pool).await[0].3.as_deref(),
            Some("trigger_removed")
        );
        assert_eq!(count(&pool, "trigger_states").await, 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn no_mail_without_queue_mail(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let cpu = [("CPU", "cpu.usage > 90")];
        let both = world(vec![host_with("db-01", &cpu), host_with("web-01", &cpu)]);
        store
            .apply_config("c1", &both, t0(), 100, true)
            .await
            .unwrap();
        for name in ["db-01", "web-01"] {
            let id = id_of(&store, name).await;
            feed(&store, id, "cpu.usage", &[], 95.0, T0_MS).await;
        }
        // The starts were mailed; closing with queue_mail off must not queue anything.
        assert_eq!(count(&pool, "notification_outbox").await, 2);
        let next = world(vec![host_with("web-01", &[])]);
        let applied = store
            .apply_config("c2", &next, later(), 100, false)
            .await
            .unwrap();
        assert_eq!(applied.problems_closed, 2);
        assert!(problems(&pool).await.iter().all(|p| p.2.is_some()));
        assert_eq!(count(&pool, "notification_outbox").await, 2);
        assert!(resolved_mail(&pool).await.is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn readded_host_starts_clean(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let cpu = [
            ("CPU", "cpu.usage > 90"),
            ("CPU silent", "cpu.usage no data for 5m"),
        ];
        let with = world(vec![host_with("web-01", &cpu)]);
        store
            .apply_config("c1", &with, t0(), 100, true)
            .await
            .unwrap();
        let id = id_of(&store, "web-01").await;
        feed(&store, id, "cpu.usage", &[], 95.0, T0_MS).await;
        store
            .touch_seen(id, t0(), &[("cpu.usage".to_owned(), Params::new())])
            .await
            .unwrap();
        store
            .apply_config("c2", &world(vec![]), later(), 100, true)
            .await
            .unwrap();
        store
            .apply_config("c3", &with, later() + Duration::minutes(1), 100, true)
            .await
            .unwrap();
        let rows = problems(&pool).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].3.as_deref(), Some("host_removed"));
        assert_eq!(count(&pool, "trigger_states").await, 0);
        assert_eq!(count(&pool, "series_seen").await, 0);
        assert_eq!(last_seen(&pool, id).await, None);
        // A value after the re-add opens a fresh problem.
        let again = feed(&store, id, "cpu.usage", &[], 95.0, T0_MS + 900_000).await;
        assert_eq!(again, 1);
        assert_eq!(problems(&pool).await.len(), 2);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn mass_removal_refusal_closes_nothing(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let cpu = [("CPU", "cpu.usage > 90")];
        let all = world((0..10).map(|i| host_with(&format!("h{i}"), &cpu)).collect());
        store
            .apply_config("c1", &all, t0(), 100, true)
            .await
            .unwrap();
        let id = id_of(&store, "h9").await;
        feed(&store, id, "cpu.usage", &[], 95.0, T0_MS).await;

        // Drops 6 hosts and the trigger of the others.
        let few = world((0..4).map(|i| host_with(&format!("h{i}"), &[])).collect());
        let err = store
            .apply_config("c2", &few, later(), 50, true)
            .await
            .unwrap_err();
        assert!(matches!(err, ApplyError::MassRemoval { .. }));
        let rows = problems(&pool).await;
        assert_eq!((rows.len(), rows[0].2, rows[0].3.clone()), (1, None, None));
        assert_eq!(count(&pool, "trigger_states").await, 1);
        assert!(resolved_mail(&pool).await.is_empty());
    }

    async fn series_of(pool: &PgPool) -> Vec<(String, String, String)> {
        sqlx::query_as(
            "SELECT h.name, s.item_key, s.instance FROM series_seen s \
             JOIN hosts h ON h.id = s.host_id ORDER BY 1, 2, 3",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    fn mount(m: &str) -> Params {
        Params::from([("mount".to_owned(), m.to_owned())])
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn series_seen_rows_of_unwatched_series_are_deleted_on_apply(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let disks = ("Disks silent", "disk.used_percent no data for 15m");
        let cpu = ("CPU silent", "cpu.usage no data for 5m");
        let root = ("Root silent", "disk.used_percent{mount=/} no data for 15m");
        let host_level = ("Agent silent", "no data for 3m");
        let apply = |commit: &'static str, web: Vec<(&'static str, &'static str)>| {
            let store = store.clone();
            async move {
                let compiled = world(vec![
                    host_with("db-01", &[disks, cpu, host_level]),
                    host_with("web-01", &web),
                ]);
                store
                    .apply_config(commit, &compiled, later(), 100, false)
                    .await
                    .unwrap();
            }
        };
        apply("c1", vec![disks, cpu, host_level]).await;
        let all = [
            ("disk.used_percent".to_owned(), mount("/")),
            ("disk.used_percent".to_owned(), mount("/data")),
            ("cpu.usage".to_owned(), Params::new()),
        ];
        for name in ["db-01", "web-01"] {
            let id = id_of(&store, name).await;
            store.touch_seen(id, t0(), &all).await.unwrap();
        }
        assert_eq!(series_of(&pool).await.len(), 6);

        // web-01 narrows its disk trigger to `/` and drops the cpu one; db-01 is unchanged.
        apply("c2", vec![root, host_level]).await;
        let web =
            |key: &str, instance: &str| ("web-01".to_owned(), key.to_owned(), instance.to_owned());
        let db =
            |key: &str, instance: &str| ("db-01".to_owned(), key.to_owned(), instance.to_owned());
        assert_eq!(
            series_of(&pool).await,
            [
                db("cpu.usage", ""),
                db("disk.used_percent", "mount=/"),
                db("disk.used_percent", "mount=/data"),
                web("disk.used_percent", "mount=/"),
            ]
        );
        // Only the host-level trigger left: the host keeps no rows.
        apply("c3", vec![host_level]).await;
        assert_eq!(series_of(&pool).await.len(), 3);
        assert!(series_of(&pool).await.iter().all(|r| r.0 == "db-01"));
        // The host's own arrival is not touched.
        assert!(
            last_seen(&pool, id_of(&store, "web-01").await)
                .await
                .is_some()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unreadable_stored_configuration_keeps_triggers_by_name(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let both = [
            ("CPU", "cpu.usage > 90"),
            ("Memory", "memory.used_percent > 90"),
        ];
        store
            .apply_config(
                "c1",
                &world(vec![host_with("web-01", &both)]),
                t0(),
                100,
                true,
            )
            .await
            .unwrap();
        let id = id_of(&store, "web-01").await;
        feed(&store, id, "cpu.usage", &[], 95.0, T0_MS).await;
        feed(&store, id, "memory.used_percent", &[], 95.0, T0_MS).await;
        // A stored configuration this version cannot read (as after a format change).
        sqlx::query("UPDATE hosts SET config = '{\"unexpected\": true}'::jsonb WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        // The new revision keeps CPU (now with another threshold) and drops Memory.
        let applied = store
            .apply_config(
                "c2",
                &world(vec![host_with("web-01", &[("CPU", "cpu.usage > 95")])]),
                later(),
                100,
                true,
            )
            .await
            .unwrap();
        assert_eq!(applied.problems_closed, 1);
        let rows = problems(&pool).await;
        let by_name: BTreeMap<&str, (Option<OffsetDateTime>, Option<&str>)> = rows
            .iter()
            .map(|r| (r.1.as_str(), (r.2, r.3.as_deref())))
            .collect();
        assert_eq!(by_name["CPU"], (None, None));
        assert_eq!(by_name["Memory"], (Some(later()), Some("trigger_removed")));
        let states: Vec<String> =
            sqlx::query_scalar("SELECT trigger_name FROM trigger_states ORDER BY 1")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(states, ["CPU"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn failing_apply_leaves_open_problems_open(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let cpu = [
            ("CPU", "cpu.usage > 90"),
            ("CPU silent", "cpu.usage no data for 5m"),
        ];
        store
            .apply_config(
                "c1",
                &world(vec![host_with("web-01", &cpu)]),
                t0(),
                100,
                true,
            )
            .await
            .unwrap();
        let id = id_of(&store, "web-01").await;
        feed(&store, id, "cpu.usage", &[], 95.0, T0_MS).await;
        store
            .touch_seen(id, t0(), &[("cpu.usage".to_owned(), Params::new())])
            .await
            .unwrap();
        // A revision that drops both triggers, but fails later in the same apply (as in
        // `apply_is_atomic`): nothing of it may stay.
        let mut broken = world(vec![host_with("web-01", &[])]);
        let dup = broken.documents[0].clone();
        broken.documents.push(dup);
        let err = store
            .apply_config("c2", &broken, later(), 100, true)
            .await
            .unwrap_err();
        assert!(matches!(err, ApplyError::State(_)));
        let rows = problems(&pool).await;
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].2, rows[0].3.as_deref()), (None, None));
        assert_eq!(count(&pool, "trigger_states").await, 1);
        assert_eq!(count(&pool, "series_seen").await, 1);
        assert!(resolved_mail(&pool).await.is_empty());
    }
}
