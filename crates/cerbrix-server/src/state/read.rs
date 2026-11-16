//! Reads for the web UI: the hosts list and the detail of one host.

use std::collections::{BTreeSet, HashMap, HashSet};

use cerbrix_common::DashboardFile;
use cerbrix_common::config::Severity;
use serde::Serialize;
use sqlx::types::Json;
use time::OffsetDateTime;
use uuid::Uuid;

use super::alerting::{Problem, ProblemRow, problem_from, problem_query};
use super::{StateError, StateStore};
use crate::gitops::EffectiveHost;
use crate::view::dashboard::selected_hosts;

/// Which hosts the list shows by their open problems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostStateFilter {
    /// At least one open problem.
    WithProblems,
    /// At least one open problem of kind `no_data`.
    NoData,
    /// No open problem.
    Ok,
}

impl HostStateFilter {
    fn as_sql(self) -> &'static str {
        match self {
            HostStateFilter::WithProblems => "problems",
            HostStateFilter::NoData => "no_data",
            HostStateFilter::Ok => "ok",
        }
    }
}

impl std::str::FromStr for HostStateFilter {
    type Err = ();

    /// The values of the `state` query parameter: `problems`, `no_data`, `ok`.
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "problems" => Ok(HostStateFilter::WithProblems),
            "no_data" => Ok(HostStateFilter::NoData),
            "ok" => Ok(HostStateFilter::Ok),
            _ => Err(()),
        }
    }
}

/// Filters and paging of the hosts list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostQuery {
    /// Case-insensitive substring of the name or the address; taken literally.
    pub search: Option<String>,
    /// An exact group name.
    pub group: Option<String>,
    pub state: Option<HostStateFilter>,
    pub limit: i64,
    pub offset: i64,
}

/// A host of the list with its open problems.
#[derive(Debug, Clone, PartialEq)]
pub struct HostRow {
    pub id: Uuid,
    pub name: String,
    pub address: Option<String>,
    pub groups: Vec<String>,
    pub source_file: Option<String>,
    pub last_seen_at: Option<OffsetDateTime>,
    pub open: Vec<Problem>,
}

/// One host with everything its detail page shows.
#[derive(Debug, Clone, PartialEq)]
pub struct HostDetail {
    pub row: HostRow,
    /// `None` for a host without stored configuration (created with `admin host add`).
    pub config: Option<EffectiveHost>,
    /// When the host enrolled; `None` when it has no credential.
    pub enrolled_at: Option<OffsetDateTime>,
    /// Names of the triggers with at least one firing instance.
    pub firing: BTreeSet<String>,
}

type HostColumns = (
    Uuid,
    String,
    Option<String>,
    Vec<String>,
    Option<String>,
    Option<OffsetDateTime>,
);

fn row_of(c: HostColumns, open: Vec<Problem>) -> HostRow {
    HostRow {
        id: c.0,
        name: c.1,
        address: c.2,
        groups: c.3,
        source_file: c.4,
        last_seen_at: c.5,
        open,
    }
}

/// Escapes `\`, `%` and `_` so a text matches itself in `ILIKE ... ESCAPE '\'`.
fn like_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The open problems per host, summarised, and the filters of the list. Shared by the page
/// and the count so both select the same hosts. Parameters: `$1` search pattern or NULL,
/// `$2` group or NULL, `$3` state filter or NULL.
macro_rules! selected_hosts {
    ($select:literal, $tail:literal) => {
        concat!(
            "WITH agg AS ( \
        SELECT host_id, \
               count(*) AS n, \
               count(*) FILTER (WHERE kind = 'no_data') AS n_nodata, \
               count(*) FILTER (WHERE acknowledged_at IS NULL) AS n_unacked, \
               max(CASE severity WHEN 'critical' THEN 3 WHEN 'high' THEN 2 \
                                 WHEN 'warning' THEN 1 ELSE 0 END) AS any_sev, \
               max(CASE severity WHEN 'critical' THEN 3 WHEN 'high' THEN 2 \
                                 WHEN 'warning' THEN 1 ELSE 0 END) \
                   FILTER (WHERE acknowledged_at IS NULL) AS unacked_sev \
        FROM problems WHERE resolved_at IS NULL GROUP BY host_id) \
    ",
            $select,
            " \
    FROM hosts h LEFT JOIN agg a ON a.host_id = h.id \
    WHERE h.active \
      AND ($1::text IS NULL OR h.name ILIKE $1 ESCAPE '\\' OR h.address ILIKE $1 ESCAPE '\\') \
      AND ($2::text IS NULL OR $2 = ANY (h.groups)) \
      AND ($3::text IS NULL \
           OR ($3 = 'problems' AND COALESCE(a.n, 0) > 0) \
           OR ($3 = 'no_data' AND COALESCE(a.n_nodata, 0) > 0) \
           OR ($3 = 'ok' AND COALESCE(a.n, 0) = 0)) ",
            $tail
        )
    };
}

impl StateStore {
    /// One page of the active hosts matching `q`, and the number of all matching hosts.
    ///
    /// Order: hosts with an unacknowledged open problem first, most severe first; then hosts
    /// whose open problems are all acknowledged, most severe first; then the others; ties by
    /// name. The page is filtered, ordered and cut in SQL; the open problems of its hosts
    /// come in one more query.
    pub async fn hosts_page(&self, q: &HostQuery) -> Result<(Vec<HostRow>, i64), StateError> {
        let search = q
            .search
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{}%", like_literal(s)));
        let state = q.state.map(HostStateFilter::as_sql);

        let total: i64 = sqlx::query_scalar(selected_hosts!("SELECT count(*)", ""))
            .bind(&search)
            .bind(&q.group)
            .bind(state)
            .fetch_one(&self.pool)
            .await?;

        let hosts: Vec<HostColumns> = sqlx::query_as(selected_hosts!(
            "SELECT h.id, h.name, h.address, h.groups, h.source_file, h.last_seen_at",
            "ORDER BY CASE WHEN COALESCE(a.n_unacked, 0) > 0 THEN 0 \
                           WHEN COALESCE(a.n, 0) > 0 THEN 1 ELSE 2 END, \
                      CASE WHEN COALESCE(a.n_unacked, 0) > 0 THEN a.unacked_sev \
                           ELSE COALESCE(a.any_sev, -1) END DESC, \
                      h.name, h.id \
             LIMIT $4 OFFSET $5"
        ))
        .bind(&search)
        .bind(&q.group)
        .bind(state)
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&self.pool)
        .await?;

        let ids: Vec<Uuid> = hosts.iter().map(|h| h.0).collect();
        let mut open = self.open_problems_of(&ids).await?;
        let rows = hosts
            .into_iter()
            .map(|c| {
                let problems = open.remove(&c.0).unwrap_or_default();
                row_of(c, problems)
            })
            .collect();
        Ok((rows, total))
    }

    /// The open problems of `host_ids`, grouped by host, each host's list oldest first.
    async fn open_problems_of(
        &self,
        host_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<Problem>>, StateError> {
        let rows: Vec<ProblemRow> = sqlx::query_as(problem_query!(
            "WHERE p.host_id = ANY($1) AND p.resolved_at IS NULL ORDER BY p.started_at, p.id"
        ))
        .bind(host_ids)
        .fetch_all(&self.pool)
        .await?;
        let mut by_host: HashMap<Uuid, Vec<Problem>> = HashMap::new();
        for row in rows {
            let problem = problem_from(row)?;
            by_host.entry(problem.host_id).or_default().push(problem);
        }
        Ok(by_host)
    }

    /// The groups of the active hosts: distinct and sorted.
    pub async fn host_groups(&self) -> Result<Vec<String>, StateError> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT g FROM hosts h, unnest(h.groups) AS g WHERE h.active ORDER BY g",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// An active host by name, with its open problems; `None` for an unknown or inactive one.
    pub async fn host_row(&self, name: &str) -> Result<Option<HostRow>, StateError> {
        let host: Option<HostColumns> = sqlx::query_as(
            "SELECT id, name, address, groups, source_file, last_seen_at \
             FROM hosts WHERE name = $1 AND active",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        let Some(host) = host else {
            return Ok(None);
        };
        let mut open = self.open_problems_of(&[host.0]).await?;
        let problems = open.remove(&host.0).unwrap_or_default();
        Ok(Some(row_of(host, problems)))
    }

    /// An active host with its stored configuration, enrolment time and firing triggers.
    pub async fn host_detail(&self, name: &str) -> Result<Option<HostDetail>, StateError> {
        let Some(row) = self.host_row(name).await? else {
            return Ok(None);
        };
        let (config, enrolled_at): (Option<Json<EffectiveHost>>, Option<OffsetDateTime>) =
            sqlx::query_as(
                "SELECT h.config, \
                        (SELECT c.enrolled_at FROM agent_credentials c WHERE c.host_id = h.id) \
                 FROM hosts h WHERE h.id = $1",
            )
            .bind(row.id)
            .fetch_one(&self.pool)
            .await?;
        let firing: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT trigger_name FROM trigger_states WHERE host_id = $1 AND firing",
        )
        .bind(row.id)
        .fetch_all(&self.pool)
        .await?;
        Ok(Some(HostDetail {
            row,
            config: config.map(|c| c.0),
            enrolled_at,
            firing: firing.into_iter().collect(),
        }))
    }
}

/// Which problems the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemStateFilter {
    /// Not resolved.
    Open,
    /// Resolved.
    Resolved,
    /// Both.
    All,
}

impl ProblemStateFilter {
    fn as_sql(self) -> &'static str {
        match self {
            ProblemStateFilter::Open => "open",
            ProblemStateFilter::Resolved => "resolved",
            ProblemStateFilter::All => "all",
        }
    }
}

impl std::str::FromStr for ProblemStateFilter {
    type Err = ();

    /// The values of the `state` query parameter: `open`, `resolved`, `all`.
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "open" => Ok(ProblemStateFilter::Open),
            "resolved" => Ok(ProblemStateFilter::Resolved),
            "all" => Ok(ProblemStateFilter::All),
            _ => Err(()),
        }
    }
}

/// Filters and paging of the problems list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemQuery {
    pub state: ProblemStateFilter,
    /// Problems of this severity and more severe ones.
    pub min_severity: Option<Severity>,
    /// An exact host name (the host may be inactive: its problems are history).
    pub host: Option<String>,
    /// An exact group name.
    pub group: Option<String>,
    /// The id of a dashboard (its file stem). The store does not look it up: the caller passes the hosts
    /// it selects to `problems_page`.
    pub dashboard: Option<String>,
    /// Case-insensitive substring of the host name, the trigger name or the summary; taken
    /// literally.
    pub search: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

/// The number of open problems of one severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SeverityCount {
    pub open: i64,
    pub unacknowledged: i64,
}

/// The open problems per severity. All four severities are always present.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ProblemCounts {
    pub critical: SeverityCount,
    pub high: SeverityCount,
    pub warning: SeverityCount,
    pub info: SeverityCount,
}

/// The active hosts by their open problems.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct HostCounts {
    pub total: i64,
    pub with_problems: i64,
    pub without_problems: i64,
    /// Active hosts that never sent a value.
    pub never_seen: i64,
}

/// The rank of a severity in SQL: critical 3, high 2, warning 1, info 0.
fn severity_rank(s: Severity) -> i32 {
    match s {
        Severity::Critical => 3,
        Severity::High => 2,
        Severity::Warning => 1,
        Severity::Info => 0,
    }
}

/// The filters of the problems list as a literal, shared by the page and the count.
/// Parameters: `$1` state (`open`, `resolved`, `all`), `$2` minimum severity rank or NULL,
/// `$3` host name or NULL, `$4` group or NULL, `$5` host ids or NULL, `$6` search pattern or
/// NULL.
macro_rules! problem_filters {
    () => {
        "WHERE (($1::text = 'open' AND p.resolved_at IS NULL) \
                OR ($1 = 'resolved' AND p.resolved_at IS NOT NULL) \
                OR $1 = 'all') \
          AND ($2::int IS NULL OR CASE p.severity WHEN 'critical' THEN 3 WHEN 'high' THEN 2 \
                                       WHEN 'warning' THEN 1 ELSE 0 END >= $2) \
          AND ($3::text IS NULL OR h.name = $3) \
          AND ($4::text IS NULL OR $4 = ANY (h.groups)) \
          AND ($5::uuid[] IS NULL OR p.host_id = ANY ($5)) \
          AND ($6::text IS NULL OR h.name ILIKE $6 ESCAPE '\\' \
               OR p.trigger_name ILIKE $6 ESCAPE '\\' OR p.summary ILIKE $6 ESCAPE '\\') "
    };
}

impl StateStore {
    /// One page of the problems matching `q`, and the number of all matching problems.
    ///
    /// `dashboard_hosts` limits the problems to those hosts when given (the hosts the
    /// dashboard named in `q.dashboard` selects, from [`StateStore::dashboard_host_ids`]).
    ///
    /// Order: open problems unacknowledged first, most severe first, oldest first; resolved
    /// ones and all: newest start first. Ties by id. Filtered, ordered and cut in SQL.
    pub async fn problems_page(
        &self,
        q: &ProblemQuery,
        dashboard_hosts: Option<&[Uuid]>,
    ) -> Result<(Vec<Problem>, i64), StateError> {
        let search = q
            .search
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{}%", like_literal(s)));
        let min = q.min_severity.map(severity_rank);
        let state = q.state.as_sql();

        let total: i64 = sqlx::query_scalar(concat!(
            "SELECT count(*) FROM problems p JOIN hosts h ON h.id = p.host_id ",
            problem_filters!()
        ))
        .bind(state)
        .bind(min)
        .bind(&q.host)
        .bind(&q.group)
        .bind(dashboard_hosts)
        .bind(&search)
        .fetch_one(&self.pool)
        .await?;

        let sql = match q.state {
            ProblemStateFilter::Open => problem_query!(concat!(
                problem_filters!(),
                "ORDER BY (p.acknowledged_at IS NOT NULL), \
                          CASE p.severity WHEN 'critical' THEN 3 WHEN 'high' THEN 2 \
                                          WHEN 'warning' THEN 1 ELSE 0 END DESC, \
                          p.started_at, p.id LIMIT $7 OFFSET $8"
            )),
            ProblemStateFilter::Resolved | ProblemStateFilter::All => problem_query!(concat!(
                problem_filters!(),
                "ORDER BY p.started_at DESC, p.id LIMIT $7 OFFSET $8"
            )),
        };
        let rows: Vec<ProblemRow> = sqlx::query_as(sql)
            .bind(state)
            .bind(min)
            .bind(&q.host)
            .bind(&q.group)
            .bind(dashboard_hosts)
            .bind(&search)
            .bind(q.limit)
            .bind(q.offset)
            .fetch_all(&self.pool)
            .await?;
        let problems = rows
            .into_iter()
            .map(problem_from)
            .collect::<Result<_, _>>()?;
        Ok((problems, total))
    }

    /// One problem, open or resolved, of an active or removed host.
    pub async fn problem(&self, id: Uuid) -> Result<Option<Problem>, StateError> {
        let row: Option<ProblemRow> = sqlx::query_as(problem_query!("WHERE p.id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(problem_from).transpose()
    }

    /// The active hosts, those with an open problem and those that never sent a value: one
    /// query.
    pub async fn host_counts(&self) -> Result<HostCounts, StateError> {
        let (total, with_problems, never_seen): (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*), \
             count(*) FILTER (WHERE EXISTS (SELECT 1 FROM problems p \
                              WHERE p.host_id = h.id AND p.resolved_at IS NULL)), \
             count(*) FILTER (WHERE h.last_seen_at IS NULL) \
             FROM hosts h WHERE h.active",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(HostCounts {
            total,
            with_problems,
            without_problems: total - with_problems,
            never_seen,
        })
    }

    /// The open problems per severity: how many, and how many of them are unacknowledged.
    pub async fn open_problem_counts(&self) -> Result<ProblemCounts, StateError> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT severity, count(*), count(*) FILTER (WHERE acknowledged_at IS NULL) \
             FROM problems WHERE resolved_at IS NULL GROUP BY severity",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut counts = ProblemCounts::default();
        for (severity, open, unacknowledged) in rows {
            let slot = match severity.as_str() {
                "critical" => &mut counts.critical,
                "high" => &mut counts.high,
                "warning" => &mut counts.warning,
                _ => &mut counts.info,
            };
            *slot = SeverityCount {
                open,
                unacknowledged,
            };
        }
        Ok(counts)
    }

    /// The ids of the active hosts the dashboard `id` selects; `None` for an unknown
    /// dashboard.
    pub async fn dashboard_host_ids(&self, id: &str) -> Result<Option<Vec<Uuid>>, StateError> {
        let Some((_, dashboard)) = self.dashboard_document(id).await? else {
            return Ok(None);
        };
        let hosts = self.active_hosts().await?;
        Ok(Some(
            selected_hosts(&dashboard, &hosts)
                .into_iter()
                .map(|h| h.id)
                .collect(),
        ))
    }

    /// Every dashboard and every active host with its open problems: three queries, however
    /// many dashboards and hosts there are. Dashboards are ordered by id, hosts by name.
    pub async fn board_inputs(&self) -> Result<BoardInputs, StateError> {
        let rows: Vec<(String, String, Json<serde_json::Value>)> = sqlx::query_as(
            "SELECT id, source_file, body FROM config_documents \
             WHERE kind = 'dashboard' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let dashboards = rows
            .into_iter()
            .map(|(id, file, body)| Ok((id, file, decode_dashboard(body.0)?)))
            .collect::<Result<_, StateError>>()?;
        let mut hosts = self.active_hosts().await?;
        let ids: Vec<Uuid> = hosts.iter().map(|h| h.id).collect();
        let mut open = self.open_problems_of(&ids).await?;
        for host in &mut hosts {
            host.open = open.remove(&host.id).unwrap_or_default();
        }
        Ok(BoardInputs { dashboards, hosts })
    }

    /// One dashboard with the active hosts it selects and their open problems: three
    /// queries. `None` for an unknown dashboard.
    pub async fn dashboard_hosts(&self, id: &str) -> Result<Option<DashboardHosts>, StateError> {
        let Some((file, dashboard)) = self.dashboard_document(id).await? else {
            return Ok(None);
        };
        let all = self.active_hosts().await?;
        let keep: HashSet<Uuid> = selected_hosts(&dashboard, &all)
            .into_iter()
            .map(|h| h.id)
            .collect();
        let mut hosts: Vec<HostRow> = all.into_iter().filter(|h| keep.contains(&h.id)).collect();
        let ids: Vec<Uuid> = hosts.iter().map(|h| h.id).collect();
        let mut open = self.open_problems_of(&ids).await?;
        for host in &mut hosts {
            host.open = open.remove(&host.id).unwrap_or_default();
        }
        Ok(Some(DashboardHosts {
            file,
            dashboard,
            hosts,
        }))
    }

    /// The stored configuration of the active hosts `names` that have one.
    pub async fn host_configs(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, EffectiveHost>, StateError> {
        let rows: Vec<(String, Json<EffectiveHost>)> = sqlx::query_as(
            "SELECT name, config FROM hosts \
             WHERE active AND name = ANY($1) AND config IS NOT NULL",
        )
        .bind(names)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(name, c)| (name, c.0)).collect())
    }

    /// The source file and content of the dashboard `id`.
    async fn dashboard_document(
        &self,
        id: &str,
    ) -> Result<Option<(String, DashboardFile)>, StateError> {
        let row: Option<(String, Json<serde_json::Value>)> = sqlx::query_as(
            "SELECT source_file, body FROM config_documents \
             WHERE kind = 'dashboard' AND id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(file, body)| Ok((file, decode_dashboard(body.0)?)))
            .transpose()
    }

    /// The active hosts, ordered by name, without their open problems.
    async fn active_hosts(&self) -> Result<Vec<HostRow>, StateError> {
        let columns: Vec<HostColumns> = sqlx::query_as(
            "SELECT id, name, address, groups, source_file, last_seen_at \
             FROM hosts WHERE active ORDER BY name, id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(columns.into_iter().map(|c| row_of(c, Vec::new())).collect())
    }
}

fn decode_dashboard(body: serde_json::Value) -> Result<DashboardFile, StateError> {
    serde_json::from_value(body).map_err(|e| StateError::Db(sqlx::Error::Decode(e.into())))
}

/// Every dashboard and every active host: what the summaries of all dashboards are made of.
#[derive(Debug, Clone)]
pub struct BoardInputs {
    /// Id, source file and content of each dashboard.
    pub dashboards: Vec<(String, String, DashboardFile)>,
    /// The active hosts with their open problems.
    pub hosts: Vec<HostRow>,
}

/// One dashboard with the hosts it selects.
#[derive(Debug, Clone)]
pub struct DashboardHosts {
    pub file: String,
    pub dashboard: DashboardFile,
    /// The selected active hosts with their open problems, ordered by name.
    pub hosts: Vec<HostRow>,
}

#[cfg(test)]
mod tests {
    use cerbrix_common::config::Severity;
    use sqlx::PgPool;

    use super::*;
    use crate::alerting::{Evaluation, OpenProblem, ProblemKind};

    const T0_MS: i64 = 1_800_000_000_000;

    fn query() -> HostQuery {
        HostQuery {
            search: None,
            group: None,
            state: None,
            limit: 50,
            offset: 0,
        }
    }

    async fn host(store: &StateStore, name: &str) -> Uuid {
        store.create_host(name).await.unwrap().id
    }

    /// Opens a problem on `host` with the trigger name given, started `age_ms` after T0.
    async fn open(
        store: &StateStore,
        host: Uuid,
        trigger: &str,
        severity: Severity,
        kind: ProblemKind,
        at_ms: i64,
    ) {
        let o = OpenProblem {
            trigger: trigger.to_owned(),
            instance: String::new(),
            severity,
            at_ms,
            summary: format!("{trigger} is bad"),
            kind,
        };
        store
            .evaluate_host(host, false, |_| Evaluation {
                open: vec![o],
                ..Evaluation::default()
            })
            .await
            .unwrap();
    }

    async fn ack(store: &StateStore, host: &str) {
        let row = store.host_row(host).await.unwrap().unwrap();
        for p in &row.open {
            store
                .acknowledge_problem(p.id, "anna", "on it", OffsetDateTime::now_utc())
                .await
                .unwrap();
        }
    }

    fn names(rows: &[HostRow]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn hosts_page_orders_problems_first(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let mut ids = std::collections::HashMap::new();
        for name in [
            "a-ok",
            "b-warn",
            "c-crit",
            "d-acked-crit",
            "e-ok",
            "f-mixed",
            "g-high",
        ] {
            ids.insert(name, host(&store, name).await);
        }
        let c = ProblemKind::Compare;
        open(&store, ids["b-warn"], "T", Severity::Warning, c, T0_MS).await;
        open(&store, ids["c-crit"], "T", Severity::Critical, c, T0_MS).await;
        open(
            &store,
            ids["d-acked-crit"],
            "T",
            Severity::Critical,
            c,
            T0_MS,
        )
        .await;
        ack(&store, "d-acked-crit").await;
        // One acknowledged critical and one unacknowledged info: unacknowledged wins.
        open(
            &store,
            ids["f-mixed"],
            "Acked",
            Severity::Critical,
            c,
            T0_MS,
        )
        .await;
        ack(&store, "f-mixed").await;
        open(
            &store,
            ids["f-mixed"],
            "Fresh",
            Severity::Info,
            c,
            T0_MS + 1,
        )
        .await;
        open(&store, ids["g-high"], "T", Severity::High, c, T0_MS).await;

        let (rows, total) = store.hosts_page(&query()).await.unwrap();
        assert_eq!(total, 7);
        assert_eq!(
            names(&rows),
            [
                // unacknowledged, most severe first
                "c-crit",
                "g-high",
                "b-warn",
                "f-mixed",
                // only acknowledged problems
                "d-acked-crit",
                // no problems, by name
                "a-ok",
                "e-ok",
            ]
        );
        let mixed = rows.iter().find(|r| r.name == "f-mixed").unwrap();
        assert_eq!(mixed.open.len(), 2);
        assert!(
            rows.iter()
                .find(|r| r.name == "a-ok")
                .unwrap()
                .open
                .is_empty()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn search_is_literal(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        for name in ["a_b", "axb", "50%", "web-01", "back\\slash"] {
            host(&store, name).await;
        }
        sqlx::query("UPDATE hosts SET address = '10.0.0.7' WHERE name = 'web-01'")
            .execute(&store.pool)
            .await
            .unwrap();
        let find = |s: &'static str| {
            let store = store.clone();
            async move {
                let q = HostQuery {
                    search: Some(s.to_owned()),
                    ..query()
                };
                let (rows, total) = store.hosts_page(&q).await.unwrap();
                assert_eq!(rows.len() as i64, total);
                rows.into_iter().map(|r| r.name).collect::<Vec<_>>()
            }
        };
        assert_eq!(find("a_b").await, ["a_b"]);
        assert_eq!(find("a_").await, ["a_b"]);
        assert_eq!(find("%").await, ["50%"]);
        assert_eq!(find("0%").await, ["50%"]);
        assert_eq!(find("\\").await, ["back\\slash"]);
        assert_eq!(find("WEB").await, ["web-01"]);
        assert_eq!(find("10.0.0.").await, ["web-01"]);
        assert!(find("zzz").await.is_empty());
        assert_eq!(
            find("").await.len(),
            5,
            "an empty search matches everything"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn filter_by_group_and_state(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let a = host(&store, "a").await;
        let b = host(&store, "b").await;
        let c = host(&store, "c").await;
        host(&store, "d").await;
        for (name, groups) in [
            ("a", vec!["prod", "web"]),
            ("b", vec!["prod"]),
            ("c", vec!["db"]),
        ] {
            sqlx::query("UPDATE hosts SET groups = $2 WHERE name = $1")
                .bind(name)
                .bind(groups.iter().map(|g| g.to_string()).collect::<Vec<_>>())
                .execute(&pool)
                .await
                .unwrap();
        }
        open(&store, a, "T", Severity::High, ProblemKind::Compare, T0_MS).await;
        open(
            &store,
            b,
            "Silent",
            Severity::High,
            ProblemKind::NoData,
            T0_MS,
        )
        .await;
        open(&store, c, "T", Severity::Info, ProblemKind::Compare, T0_MS).await;

        let ask = |group: Option<&str>, state: Option<HostStateFilter>| {
            let store = store.clone();
            let q = HostQuery {
                group: group.map(str::to_owned),
                state,
                ..query()
            };
            async move {
                let (rows, total) = store.hosts_page(&q).await.unwrap();
                let mut names: Vec<_> = rows.into_iter().map(|r| r.name).collect();
                names.sort();
                (names, total)
            }
        };
        assert_eq!(
            ask(Some("prod"), None).await,
            (vec!["a".into(), "b".into()], 2)
        );
        assert_eq!(ask(Some("web"), None).await.0, ["a"]);
        assert_eq!(ask(Some("pro"), None).await.1, 0, "a group matches exactly");
        assert_eq!(ask(Some("nope"), None).await.1, 0);
        assert_eq!(
            ask(None, Some(HostStateFilter::WithProblems)).await.0,
            ["a", "b", "c"]
        );
        assert_eq!(ask(None, Some(HostStateFilter::NoData)).await.0, ["b"]);
        assert_eq!(ask(None, Some(HostStateFilter::Ok)).await.0, ["d"]);
        assert_eq!(
            ask(Some("prod"), Some(HostStateFilter::NoData)).await.0,
            ["b"]
        );
        assert_eq!(ask(Some("db"), Some(HostStateFilter::Ok)).await.1, 0);
        assert_eq!(store.host_groups().await.unwrap(), ["db", "prod", "web"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn paging_and_total(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        for i in 0..7 {
            host(&store, &format!("h{i}")).await;
        }
        let page = |limit, offset| {
            let store = store.clone();
            async move {
                let q = HostQuery {
                    limit,
                    offset,
                    ..query()
                };
                let (rows, total) = store.hosts_page(&q).await.unwrap();
                (rows.into_iter().map(|r| r.name).collect::<Vec<_>>(), total)
            }
        };
        assert_eq!(
            page(3, 0).await,
            (vec!["h0".to_owned(), "h1".into(), "h2".into()], 7)
        );
        assert_eq!(
            page(3, 3).await,
            (vec!["h3".to_owned(), "h4".into(), "h5".into()], 7)
        );
        assert_eq!(page(3, 6).await, (vec!["h6".to_owned()], 7));
        assert_eq!(page(3, 30).await, (Vec::<String>::new(), 7));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn inactive_hosts_are_not_listed(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let gone = host(&store, "gone").await;
        host(&store, "here").await;
        open(
            &store,
            gone,
            "T",
            Severity::High,
            ProblemKind::Compare,
            T0_MS,
        )
        .await;
        store.set_host_active(gone, false).await.unwrap();
        let (rows, total) = store.hosts_page(&query()).await.unwrap();
        assert_eq!((names(&rows), total), (vec!["here"], 1));
        let q = HostQuery {
            state: Some(HostStateFilter::WithProblems),
            ..query()
        };
        assert_eq!(store.hosts_page(&q).await.unwrap().1, 0);
        assert_eq!(store.host_row("gone").await.unwrap(), None);
        assert_eq!(store.host_row("nope").await.unwrap(), None);
        assert!(store.host_row("here").await.unwrap().is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn page_of_200_hosts_with_problems_is_correct(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        for i in 0..200 {
            let id = host(&store, &format!("host-{i:03}")).await;
            for t in 0..3 {
                let severity = [Severity::Info, Severity::High, Severity::Critical][t];
                open(
                    &store,
                    id,
                    &format!("T{i}-{t}"),
                    severity,
                    ProblemKind::Compare,
                    T0_MS + t as i64,
                )
                .await;
            }
        }
        let q = HostQuery {
            limit: 200,
            ..query()
        };
        let (rows, total) = store.hosts_page(&q).await.unwrap();
        assert_eq!((rows.len(), total), (200, 200));
        for row in &rows {
            let i: usize = row.name.strip_prefix("host-").unwrap().parse().unwrap();
            let mut triggers: Vec<_> = row.open.iter().map(|p| p.trigger.clone()).collect();
            triggers.sort();
            assert_eq!(
                triggers,
                (0..3).map(|t| format!("T{i}-{t}")).collect::<Vec<_>>(),
                "{}",
                row.name
            );
            assert!(
                row.open
                    .iter()
                    .all(|p| p.host_id == row.id && p.host == row.name)
            );
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn detail_of_a_host_without_configuration(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let id = host(&store, "bare").await;
        let d = store.host_detail("bare").await.unwrap().unwrap();
        assert_eq!(d.row.id, id);
        assert_eq!((d.config, d.enrolled_at), (None, None));
        assert!(d.firing.is_empty());
        let cred = store.bind_credential(id).await.unwrap();
        assert!(!cred.is_empty());
        assert!(
            store
                .host_detail("bare")
                .await
                .unwrap()
                .unwrap()
                .enrolled_at
                .is_some()
        );
        assert_eq!(store.host_detail("nope").await.unwrap(), None);
    }

    fn pquery() -> ProblemQuery {
        ProblemQuery {
            state: ProblemStateFilter::Open,
            min_severity: None,
            host: None,
            group: None,
            dashboard: None,
            search: None,
            limit: 50,
            offset: 0,
        }
    }

    /// The trigger names of a page, in order.
    async fn triggers(store: &StateStore, q: &ProblemQuery) -> Vec<String> {
        let (rows, total) = store.problems_page(q, None).await.unwrap();
        assert!(total >= rows.len() as i64);
        rows.into_iter().map(|p| p.trigger).collect()
    }

    async fn ack_trigger(store: &StateStore, host: &str, trigger: &str) {
        let row = store.host_row(host).await.unwrap().unwrap();
        let p = row.open.iter().find(|p| p.trigger == trigger).unwrap();
        store
            .acknowledge_problem(p.id, "anna", "on it", OffsetDateTime::now_utc())
            .await
            .unwrap();
    }

    async fn close_trigger(store: &StateStore, host: &str, trigger: &str) {
        let row = store.host_row(host).await.unwrap().unwrap();
        let p = row.open.iter().find(|p| p.trigger == trigger).unwrap();
        store
            .close_problem(p.id, "anna", "gone", OffsetDateTime::now_utc(), false)
            .await
            .unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn open_problems_order(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let h = host(&store, "h").await;
        let c = ProblemKind::Compare;
        // Unacknowledged: most severe first, equal severity oldest first.
        open(&store, h, "info", Severity::Info, c, T0_MS).await;
        open(&store, h, "high-new", Severity::High, c, T0_MS + 20).await;
        open(&store, h, "warn", Severity::Warning, c, T0_MS).await;
        open(&store, h, "high-old", Severity::High, c, T0_MS + 10).await;
        open(&store, h, "crit", Severity::Critical, c, T0_MS + 99).await;
        // Acknowledged ones come last, in the same order among themselves.
        open(&store, h, "acked-info", Severity::Info, c, T0_MS).await;
        open(&store, h, "acked-crit", Severity::Critical, c, T0_MS).await;
        ack_trigger(&store, "h", "acked-info").await;
        ack_trigger(&store, "h", "acked-crit").await;
        assert_eq!(
            triggers(&store, &pquery()).await,
            [
                "crit",
                "high-old",
                "high-new",
                "warn",
                "info",
                "acked-crit",
                "acked-info"
            ]
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn filter_by_min_severity_host_group(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let a = host(&store, "a").await;
        let b = host(&store, "b").await;
        sqlx::query("UPDATE hosts SET groups = ARRAY['prod','web'] WHERE name = 'a'")
            .execute(&pool)
            .await
            .unwrap();
        let c = ProblemKind::Compare;
        open(&store, a, "a-crit", Severity::Critical, c, T0_MS).await;
        open(&store, a, "a-high", Severity::High, c, T0_MS + 1).await;
        open(&store, a, "a-warn", Severity::Warning, c, T0_MS + 2).await;
        open(&store, b, "b-info", Severity::Info, c, T0_MS).await;
        open(&store, b, "b-high", Severity::High, c, T0_MS + 5).await;
        let min = |s| ProblemQuery {
            min_severity: Some(s),
            ..pquery()
        };
        assert_eq!(
            triggers(&store, &min(Severity::High)).await,
            ["a-crit", "a-high", "b-high"]
        );
        assert_eq!(triggers(&store, &min(Severity::Critical)).await, ["a-crit"]);
        assert_eq!(triggers(&store, &min(Severity::Info)).await.len(), 5);
        let on = |h: &str| ProblemQuery {
            host: Some(h.to_owned()),
            ..pquery()
        };
        assert_eq!(triggers(&store, &on("b")).await, ["b-high", "b-info"]);
        assert!(triggers(&store, &on("B")).await.is_empty(), "exact name");
        let group = |g: &str| ProblemQuery {
            group: Some(g.to_owned()),
            ..pquery()
        };
        assert_eq!(triggers(&store, &group("web")).await.len(), 3);
        assert!(triggers(&store, &group("we")).await.is_empty());
        let all = ProblemQuery {
            group: Some("prod".into()),
            min_severity: Some(Severity::High),
            host: Some("a".into()),
            ..pquery()
        };
        assert_eq!(triggers(&store, &all).await, ["a-crit", "a-high"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn filter_by_dashboard_hosts(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let a = host(&store, "a").await;
        let b = host(&store, "b").await;
        let gone = host(&store, "gone").await;
        sqlx::query("UPDATE hosts SET groups = ARRAY['prod'] WHERE name IN ('a', 'gone')")
            .execute(&pool)
            .await
            .unwrap();
        for (id, body) in [
            ("prod", r#"{"name": "prod", "hosts": {"groups": ["prod"]}}"#),
            ("b-only", r#"{"name": "b", "hosts": {"names": ["b"]}}"#),
            ("all", r#"{"name": "all"}"#),
        ] {
            sqlx::query(
                "INSERT INTO config_documents (kind, id, source_file, body) \
                 VALUES ('dashboard', $1, 'd.json', $2::jsonb)",
            )
            .bind(id)
            .bind(body)
            .execute(&pool)
            .await
            .unwrap();
        }
        let c = ProblemKind::Compare;
        open(&store, a, "on-a", Severity::High, c, T0_MS).await;
        open(&store, b, "on-b", Severity::High, c, T0_MS + 1).await;
        open(&store, gone, "on-gone", Severity::High, c, T0_MS + 2).await;
        store.set_host_active(gone, false).await.unwrap();

        // The inactive host is not selected, though its problem is still listed.
        assert_eq!(
            store.dashboard_host_ids("prod").await.unwrap().unwrap(),
            [a]
        );
        assert_eq!(
            store.dashboard_host_ids("b-only").await.unwrap().unwrap(),
            [b]
        );
        let mut every = store.dashboard_host_ids("all").await.unwrap().unwrap();
        every.sort();
        let mut expected = vec![a, b];
        expected.sort();
        assert_eq!(every, expected);
        assert_eq!(store.dashboard_host_ids("nope").await.unwrap(), None);

        let q = ProblemQuery {
            dashboard: Some("prod".into()),
            ..pquery()
        };
        let (rows, total) = store.problems_page(&q, Some(&[a])).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].trigger, "on-a");
        let (rows, total) = store.problems_page(&q, Some(&[])).await.unwrap();
        assert_eq!((rows.len(), total), (0, 0));
        assert_eq!(triggers(&store, &pquery()).await.len(), 3);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn search_is_literal_across_host_trigger_summary(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let plain = host(&store, "plain").await;
        let odd = host(&store, "web_01").await;
        let c = ProblemKind::Compare;
        open(&store, plain, "Disk 100% full", Severity::High, c, T0_MS).await;
        open(&store, plain, "Cpu", Severity::High, c, T0_MS + 1).await;
        open(&store, odd, "Mem", Severity::High, c, T0_MS + 2).await;
        sqlx::query(
            "UPDATE problems SET summary = 'path C:\\tmp is gone' WHERE trigger_name = 'Cpu'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let find = |s: &str| {
            let store = store.clone();
            let q = ProblemQuery {
                search: Some(s.to_owned()),
                ..pquery()
            };
            async move { triggers(&store, &q).await }
        };
        assert_eq!(find("web_01").await, ["Mem"], "host name");
        assert_eq!(find("WEB_").await, ["Mem"], "case-insensitive");
        assert!(find("webx01").await.is_empty(), "_ is not a wildcard");
        assert_eq!(find("100%").await, ["Disk 100% full"], "trigger name");
        assert_eq!(find("%").await, ["Disk 100% full"], "% is literal");
        assert_eq!(find("c:\\tmp").await, ["Cpu"], "summary, backslash literal");
        assert_eq!(find("is bad").await.len(), 2, "summary of the others");
        assert_eq!(
            find("").await.len(),
            3,
            "an empty search matches everything"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn resolved_are_newest_first(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let h = host(&store, "h").await;
        let c = ProblemKind::Compare;
        for (t, at) in [("old", 0), ("new", 20), ("mid", 10)] {
            open(&store, h, t, Severity::High, c, T0_MS + at).await;
        }
        open(&store, h, "still-open", Severity::Critical, c, T0_MS + 30).await;
        for t in ["old", "new", "mid"] {
            close_trigger(&store, "h", t).await;
        }
        let q = |state| ProblemQuery { state, ..pquery() };
        assert_eq!(
            triggers(&store, &q(ProblemStateFilter::Resolved)).await,
            ["new", "mid", "old"]
        );
        assert_eq!(
            triggers(&store, &q(ProblemStateFilter::All)).await,
            ["still-open", "new", "mid", "old"]
        );
        assert_eq!(
            triggers(&store, &q(ProblemStateFilter::Open)).await,
            ["still-open"]
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn problem_paging_and_total(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let h = host(&store, "h").await;
        for i in 0..7 {
            let c = ProblemKind::Compare;
            open(&store, h, &format!("t{i}"), Severity::High, c, T0_MS + i).await;
        }
        let page = |limit, offset| {
            let store = store.clone();
            async move {
                let q = ProblemQuery {
                    limit,
                    offset,
                    ..pquery()
                };
                let (rows, total) = store.problems_page(&q, None).await.unwrap();
                (
                    rows.into_iter().map(|p| p.trigger).collect::<Vec<_>>(),
                    total,
                )
            }
        };
        assert_eq!(
            page(3, 0).await,
            (vec!["t0".to_owned(), "t1".into(), "t2".into()], 7)
        );
        assert_eq!(page(3, 6).await, (vec!["t6".to_owned()], 7));
        assert_eq!(page(3, 30).await, (Vec::<String>::new(), 7));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn one_problem_by_id(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let h = host(&store, "h").await;
        open(&store, h, "T", Severity::High, ProblemKind::Compare, T0_MS).await;
        let id = store.host_row("h").await.unwrap().unwrap().open[0].id;
        let p = store.problem(id).await.unwrap().unwrap();
        assert_eq!((p.id, p.host.as_str(), p.trigger.as_str()), (id, "h", "T"));
        assert_eq!(store.problem(Uuid::new_v4()).await.unwrap(), None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn counts_per_severity(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let h = host(&store, "h").await;
        let c = ProblemKind::Compare;
        let zero = serde_json::json!({"open": 0, "unacknowledged": 0});
        let empty = serde_json::to_value(store.open_problem_counts().await.unwrap()).unwrap();
        assert_eq!(
            empty,
            serde_json::json!({
                "critical": zero, "high": zero, "warning": zero, "info": zero
            })
        );
        open(&store, h, "c1", Severity::Critical, c, T0_MS).await;
        open(&store, h, "c2", Severity::Critical, c, T0_MS).await;
        for t in ["h1", "h2", "h3"] {
            open(&store, h, t, Severity::High, c, T0_MS).await;
        }
        ack_trigger(&store, "h", "h1").await;
        ack_trigger(&store, "h", "h2").await;
        // A resolved problem is not counted.
        close_trigger(&store, "h", "c2").await;
        let v = serde_json::to_value(store.open_problem_counts().await.unwrap()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "critical": {"open": 1, "unacknowledged": 1},
                "high": {"open": 3, "unacknowledged": 1},
                "warning": zero,
                "info": zero,
            })
        );
    }
}
