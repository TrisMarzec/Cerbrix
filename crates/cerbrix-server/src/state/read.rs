//! Reads for the web UI: the hosts list and the detail of one host.

use std::collections::{BTreeSet, HashMap};

use sqlx::types::Json;
use time::OffsetDateTime;
use uuid::Uuid;

use super::alerting::{Problem, ProblemRow, problem_from, problem_query};
use super::{StateError, StateStore};
use crate::gitops::EffectiveHost;

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
}
