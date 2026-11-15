//! Trigger state, problems and the notification outbox.

use std::collections::BTreeMap;

use cerbrix_common::config::Severity;
use cerbrix_common::{NotificationFile, TriggerExpr, parse_trigger};
use sqlx::types::Json;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::{StateError, StateStore};
use crate::alerting::nodata::Seen;
use crate::alerting::{
    Evaluation, EventKind, Notification, OpenProblem, Params, ResolveProblem, TriggerState,
    instance_key, recipients,
};
use crate::gitops::EffectiveHost;

/// How long a claimed outbox row stays reserved for its claimer.
const LEASE: Duration = Duration::minutes(5);
const MAX_ERROR_CHARS: usize = 500;
const MAX_ACK_BY_CHARS: usize = 200;
const MAX_ACK_COMMENT_CHARS: usize = 1000;

/// A problem as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub id: Uuid,
    pub host_id: Uuid,
    pub host: String,
    pub trigger: String,
    pub instance: String,
    pub severity: Severity,
    pub summary: String,
    pub started_at: OffsetDateTime,
    pub resolved_at: Option<OffsetDateTime>,
    pub resolve_reason: Option<String>,
    pub acknowledged_at: Option<OffsetDateTime>,
    pub acknowledged_by: Option<String>,
    pub ack_comment: Option<String>,
}

/// What `decide` of [`StateStore::evaluate_host`] sees.
#[derive(Debug, Clone)]
pub struct AlertContext {
    pub host: String,
    pub groups: Vec<String>,
    /// `None` for a host without configuration from Git.
    pub config: Option<EffectiveHost>,
    /// Every stored trigger state of the host.
    pub states: Vec<TriggerState>,
}

/// A problem that was actually opened or resolved by an evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProblemEvent {
    pub problem_id: Uuid,
    pub kind: EventKind,
}

/// A claimed outbox row with the notification to send.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxItem {
    pub id: i64,
    pub recipient: String,
    /// Delivery attempts including this one.
    pub attempts: i32,
    pub created_at: OffsetDateTime,
    pub notification: Notification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemFilter {
    /// Unresolved problems, most severe first, then oldest first.
    Open,
    /// All problems, newest first.
    All,
}

fn encode_err(e: impl std::error::Error + Send + Sync + 'static) -> StateError {
    StateError::Db(sqlx::Error::Encode(Box::new(e)))
}

fn decode_err(what: &str, value: &str) -> StateError {
    StateError::Db(sqlx::Error::Decode(
        format!("unknown {what} {value:?}").into(),
    ))
}

/// Unix milliseconds to a timestamp.
fn from_ms(ms: i64) -> Result<OffsetDateTime, StateError> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).map_err(encode_err)
}

/// A timestamp to Unix milliseconds (PostgreSQL keeps microseconds, so this is exact).
fn to_ms(t: OffsetDateTime) -> i64 {
    t.unix_timestamp_nanos().div_euclid(1_000_000) as i64
}

fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::High => "high",
        Severity::Critical => "critical",
    }
}

fn parse_severity(s: &str) -> Result<Severity, StateError> {
    match s {
        "info" => Ok(Severity::Info),
        "warning" => Ok(Severity::Warning),
        "high" => Ok(Severity::High),
        "critical" => Ok(Severity::Critical),
        other => Err(decode_err("severity", other)),
    }
}

fn parse_event(s: &str) -> Result<EventKind, StateError> {
    match s {
        "started" => Ok(EventKind::Started),
        "resolved" => Ok(EventKind::Resolved),
        other => Err(decode_err("event", other)),
    }
}

/// Removes control characters (PostgreSQL also rejects NUL) and cuts to `max` characters.
fn cleaned(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// Cuts to `max` characters; keeps line breaks but drops NUL, which PostgreSQL rejects.
fn cut(s: &str, max: usize) -> String {
    s.chars().filter(|c| *c != '\0').take(max).collect()
}

type ProblemRow = (
    Uuid,
    Uuid,
    String,
    String,
    String,
    String,
    String,
    OffsetDateTime,
    Option<OffsetDateTime>,
    Option<String>,
    Option<OffsetDateTime>,
    Option<String>,
    Option<String>,
);

/// The problem query with a WHERE/ORDER BY tail appended; a macro so the result stays a
/// literal SQL string.
macro_rules! problem_query {
    ($tail:literal) => {
        concat!(
            "SELECT p.id, p.host_id, h.name, p.trigger_name, p.instance, \
             p.severity, p.summary, p.started_at, p.resolved_at, p.resolve_reason, \
             p.acknowledged_at, p.acknowledged_by, p.ack_comment \
             FROM problems p JOIN hosts h ON h.id = p.host_id ",
            $tail
        )
    };
}

fn problem_from(r: ProblemRow) -> Result<Problem, StateError> {
    Ok(Problem {
        id: r.0,
        host_id: r.1,
        host: r.2,
        trigger: r.3,
        instance: r.4,
        severity: parse_severity(&r.5)?,
        summary: r.6,
        started_at: r.7,
        resolved_at: r.8,
        resolve_reason: r.9,
        acknowledged_at: r.10,
        acknowledged_by: r.11,
        ack_comment: r.12,
    })
}

type StateRow = (
    String,
    String,
    bool,
    Option<OffsetDateTime>,
    OffsetDateTime,
    Option<f64>,
);

type ClaimRow = (
    i64,
    String,
    i32,
    OffsetDateTime,
    String,
    String,
    String,
    String,
    String,
    String,
    OffsetDateTime,
    Option<OffsetDateTime>,
    Option<String>,
);

/// An open or resolve of an evaluation, ordered by time.
enum Action<'a> {
    Open(&'a OpenProblem),
    Resolve(&'a ResolveProblem),
}

/// The notification rules in the database; a document that does not parse is skipped.
async fn load_rules(tx: &mut sqlx::PgConnection) -> Result<Vec<NotificationFile>, StateError> {
    let docs: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT id, body FROM config_documents WHERE kind = 'notification' ORDER BY id",
    )
    .fetch_all(&mut *tx)
    .await?;
    Ok(docs
        .into_iter()
        .filter_map(
            |(id, body)| match serde_json::from_value::<NotificationFile>(body) {
                Ok(rule) => Some(rule),
                Err(e) => {
                    tracing::error!(rule = %id, error = %e, "skipping unreadable notification rule");
                    None
                }
            },
        )
        .collect())
}

impl StateStore {
    /// Locks the host row, loads the context, runs `decide`, and applies its result:
    /// state upserts, problems opened and resolved, outbox rows. One transaction.
    ///
    /// An inactive or unknown host yields no events and `decide` is not called. An event is
    /// reported only for a problem that was actually opened or resolved. If `decide`
    /// panics the transaction is rolled back and the panic propagates.
    pub async fn evaluate_host<F>(
        &self,
        host_id: Uuid,
        queue_mail: bool,
        decide: F,
    ) -> Result<Vec<ProblemEvent>, StateError>
    where
        F: FnOnce(&AlertContext) -> Evaluation + Send,
    {
        let mut tx = self.pool.begin().await?;
        let host: Option<(String, Vec<String>, Option<Json<EffectiveHost>>)> = sqlx::query_as(
            "SELECT name, groups, config FROM hosts WHERE id = $1 AND active FOR UPDATE",
        )
        .bind(host_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((name, groups, config)) = host else {
            return Ok(Vec::new());
        };

        let rows: Vec<StateRow> = sqlx::query_as(
            "SELECT trigger_name, instance, firing, condition_since, last_ts, last_value \
             FROM trigger_states WHERE host_id = $1 ORDER BY trigger_name, instance",
        )
        .bind(host_id)
        .fetch_all(&mut *tx)
        .await?;
        let ctx = AlertContext {
            host: name,
            groups,
            config: config.map(|c| c.0),
            states: rows
                .into_iter()
                .map(|r| TriggerState {
                    trigger: r.0,
                    instance: r.1,
                    firing: r.2,
                    condition_since_ms: r.3.map(to_ms),
                    last_ts_ms: to_ms(r.4),
                    last_value: r.5,
                })
                .collect(),
        };

        let evaluation = decide(&ctx);

        write_states(&mut tx, host_id, &evaluation.states).await?;

        // Opens and resolves in time order; at the same instant an open comes first.
        let mut actions: Vec<(i64, u8, Action<'_>)> = evaluation
            .open
            .iter()
            .map(|o| (o.at_ms, 0, Action::Open(o)))
            .chain(
                evaluation
                    .resolve
                    .iter()
                    .map(|r| (r.at_ms, 1, Action::Resolve(r))),
            )
            .collect();
        actions.sort_by_key(|(at, order, _)| (*at, *order));

        let mut events = Vec::new();
        let mut rules: Option<Vec<NotificationFile>> = None;
        for (_, _, action) in actions {
            match action {
                Action::Open(o) => {
                    let started = from_ms(o.at_ms)?;
                    let inserted: Option<Uuid> = sqlx::query_scalar(
                        "INSERT INTO problems \
                         (id, host_id, trigger_name, instance, severity, summary, started_at) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7) \
                         ON CONFLICT (host_id, trigger_name, instance) \
                         WHERE resolved_at IS NULL DO NOTHING RETURNING id",
                    )
                    .bind(Uuid::new_v4())
                    .bind(host_id)
                    .bind(&o.trigger)
                    .bind(&o.instance)
                    .bind(severity_str(o.severity))
                    .bind(&o.summary)
                    .bind(started)
                    .fetch_optional(&mut *tx)
                    .await?;
                    let Some(problem_id) = inserted else {
                        continue;
                    };
                    events.push(ProblemEvent {
                        problem_id,
                        kind: EventKind::Started,
                    });
                    if queue_mail {
                        if rules.is_none() {
                            rules = Some(load_rules(&mut tx).await?);
                        }
                        let to =
                            recipients(rules.as_deref().unwrap_or(&[]), o.severity, &ctx.groups);
                        if !to.is_empty() {
                            sqlx::query(
                                "INSERT INTO notification_outbox \
                                 (problem_id, event, recipient, created_at, next_attempt_at) \
                                 SELECT $1, 'started', r, $3, $3 FROM UNNEST($2::text[]) AS r \
                                 ON CONFLICT (problem_id, event, recipient) DO NOTHING",
                            )
                            .bind(problem_id)
                            .bind(&to)
                            .bind(started)
                            .execute(&mut *tx)
                            .await?;
                        }
                    }
                }
                Action::Resolve(r) => {
                    let ended = from_ms(r.at_ms)?;
                    let updated: Option<Uuid> = sqlx::query_scalar(
                        "UPDATE problems SET resolved_at = GREATEST($4, started_at), \
                         resolve_reason = 'condition_ended' \
                         WHERE host_id = $1 AND trigger_name = $2 AND instance = $3 \
                         AND resolved_at IS NULL RETURNING id",
                    )
                    .bind(host_id)
                    .bind(&r.trigger)
                    .bind(&r.instance)
                    .bind(ended)
                    .fetch_optional(&mut *tx)
                    .await?;
                    let Some(problem_id) = updated else {
                        continue;
                    };
                    events.push(ProblemEvent {
                        problem_id,
                        kind: EventKind::Resolved,
                    });
                    if queue_mail {
                        // Tell exactly those who were told about the start.
                        sqlx::query(
                            "INSERT INTO notification_outbox \
                             (problem_id, event, recipient, created_at, next_attempt_at) \
                             SELECT problem_id, 'resolved', recipient, $2, $2 \
                             FROM notification_outbox WHERE problem_id = $1 AND event = 'started' \
                             ON CONFLICT (problem_id, event, recipient) DO NOTHING",
                        )
                        .bind(problem_id)
                        .bind(ended)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            }
        }
        tx.commit().await?;
        Ok(events)
    }

    /// Records that values arrived at `now`: sets `hosts.last_seen_at` and upserts
    /// `series_seen` for the given item keys and parameter maps. Neither value ever moves
    /// backwards, so concurrent calls from several instances cannot undo each other.
    pub async fn touch_seen(
        &self,
        host_id: Uuid,
        now: OffsetDateTime,
        series: &[(String, Params)],
    ) -> Result<(), StateError> {
        let mut unique: BTreeMap<(&str, String), &Params> = BTreeMap::new();
        for (key, params) in series {
            unique.insert((key.as_str(), instance_key(params)), params);
        }
        let mut keys = Vec::with_capacity(unique.len());
        let mut instances = Vec::with_capacity(unique.len());
        let mut params = Vec::with_capacity(unique.len());
        for ((key, instance), p) in unique {
            keys.push(key.to_owned());
            instances.push(instance);
            params.push(serde_json::to_value(p).map_err(encode_err)?);
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE hosts SET last_seen_at = GREATEST(COALESCE(last_seen_at, $2), $2) \
             WHERE id = $1",
        )
        .bind(host_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if !keys.is_empty() {
            sqlx::query(
                "INSERT INTO series_seen (host_id, item_key, instance, params, last_arrival) \
                 SELECT $1, t.k, t.i, t.p, $5 \
                 FROM UNNEST($2::text[], $3::text[], $4::jsonb[]) AS t(k, i, p) \
                 ON CONFLICT (host_id, item_key, instance) DO UPDATE SET \
                     last_arrival = GREATEST(series_seen.last_arrival, EXCLUDED.last_arrival)",
            )
            .bind(host_id)
            .bind(&keys)
            .bind(&instances)
            .bind(&params)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Records that the no-data check is running and returns the baseline: the time since
    /// which silence counts. If the previous heartbeat is missing or older than
    /// 3 x `check_every` the server was not checking (a gap), and the baseline moves to
    /// `now`; otherwise it stays. One statement, so instances that call it at the same
    /// moment cannot both see a gap and set different baselines.
    pub async fn alerting_heartbeat(
        &self,
        now: OffsetDateTime,
        check_every: std::time::Duration,
    ) -> Result<Option<OffsetDateTime>, StateError> {
        let tolerance = Duration::try_from(check_every)
            .unwrap_or(Duration::MAX)
            .saturating_mul(3);
        let stale_before = now
            .checked_sub(tolerance)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let baseline: Option<OffsetDateTime> = sqlx::query_scalar(
            "UPDATE alerting_state SET \
                 baseline_at = CASE WHEN heartbeat_at IS NULL OR heartbeat_at < $2 \
                                    THEN $1 ELSE baseline_at END, \
                 heartbeat_at = GREATEST(heartbeat_at, $1) \
             WHERE singleton RETURNING baseline_at",
        )
        .bind(now)
        .bind(stale_before)
        .fetch_one(&self.pool)
        .await?;
        Ok(baseline)
    }

    /// Active hosts whose effective triggers include a no-data trigger, ordered by name.
    /// A stored configuration that cannot be read is skipped and logged.
    pub async fn hosts_with_nodata_triggers(&self) -> Result<Vec<Uuid>, StateError> {
        let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT id, config FROM hosts WHERE active AND config IS NOT NULL ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, config)| {
                let host = match serde_json::from_value::<EffectiveHost>(config) {
                    Ok(host) => host,
                    Err(err) => {
                        tracing::error!(host_id = %id, %err, "skipping unreadable host configuration");
                        return None;
                    }
                };
                host.triggers
                    .iter()
                    .any(|t| matches!(parse_trigger(&t.expression), Ok(TriggerExpr::NoData { .. })))
                    .then_some(id)
            })
            .collect())
    }

    /// When values last arrived for the host and for each of its watched series.
    pub async fn seen(&self, host_id: Uuid) -> Result<Seen, StateError> {
        let last_seen: Option<OffsetDateTime> =
            sqlx::query_scalar("SELECT last_seen_at FROM hosts WHERE id = $1")
                .bind(host_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        let rows: Vec<(String, Json<Params>, OffsetDateTime)> = sqlx::query_as(
            "SELECT item_key, params, last_arrival FROM series_seen \
             WHERE host_id = $1 ORDER BY item_key, instance",
        )
        .bind(host_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(Seen {
            last_seen_ms: last_seen.map(to_ms),
            series: rows
                .into_iter()
                .map(|(key, params, at)| (key, params.0, to_ms(at)))
                .collect(),
        })
    }

    /// Problems, at most `limit`: see [`ProblemFilter`] for the order.
    pub async fn problems(
        &self,
        filter: ProblemFilter,
        limit: i64,
    ) -> Result<Vec<Problem>, StateError> {
        let sql = match filter {
            ProblemFilter::Open => problem_query!(
                "WHERE p.resolved_at IS NULL ORDER BY CASE p.severity \
                 WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'warning' THEN 2 ELSE 3 END, \
                 p.started_at, p.id LIMIT $1"
            ),
            ProblemFilter::All => {
                problem_query!("ORDER BY p.started_at DESC, p.id LIMIT $1")
            }
        };
        let rows: Vec<ProblemRow> = sqlx::query_as(sql)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(problem_from).collect()
    }

    /// Records who acknowledged a problem; acknowledging again overwrites. `NotFound` for an
    /// unknown id.
    pub async fn acknowledge_problem(
        &self,
        id: Uuid,
        by: &str,
        comment: &str,
        now: OffsetDateTime,
    ) -> Result<Problem, StateError> {
        let done = sqlx::query(
            "UPDATE problems SET acknowledged_at = $2, acknowledged_by = $3, ack_comment = $4 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(now)
        .bind(cut(by, MAX_ACK_BY_CHARS))
        .bind(cut(comment, MAX_ACK_COMMENT_CHARS))
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StateError::NotFound);
        }
        let row: Option<ProblemRow> = sqlx::query_as(problem_query!("WHERE p.id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        problem_from(row.ok_or(StateError::NotFound)?)
    }

    /// Reserves up to `limit` due outbox rows for five minutes and returns them, oldest
    /// first. Safe when several instances call it at the same moment: a row goes to one
    /// caller. A `resolved` row waits while the `started` row for the same problem and
    /// recipient is still unsent and not failed.
    pub async fn claim_notifications(
        &self,
        now: OffsetDateTime,
        limit: i64,
    ) -> Result<Vec<OutboxItem>, StateError> {
        let rows: Vec<ClaimRow> = sqlx::query_as(
            "WITH claimed AS ( \
                 UPDATE notification_outbox o \
                 SET locked_until = $2, attempts = o.attempts + 1 \
                 WHERE o.id IN ( \
                     SELECT n.id FROM notification_outbox n \
                     WHERE n.sent_at IS NULL AND n.failed_at IS NULL \
                       AND n.next_attempt_at <= $1 \
                       AND (n.locked_until IS NULL OR n.locked_until < $1) \
                       AND (n.event = 'started' OR NOT EXISTS ( \
                           SELECT 1 FROM notification_outbox s \
                           WHERE s.problem_id = n.problem_id AND s.recipient = n.recipient \
                             AND s.event = 'started' \
                             AND s.sent_at IS NULL AND s.failed_at IS NULL)) \
                     ORDER BY n.id LIMIT $3 FOR UPDATE SKIP LOCKED) \
                 RETURNING o.id, o.problem_id, o.event, o.recipient, o.attempts, o.created_at) \
             SELECT c.id, c.recipient, c.attempts, c.created_at, c.event, h.name, \
                    p.trigger_name, p.instance, p.severity, p.summary, p.started_at, \
                    p.resolved_at, p.resolve_reason \
             FROM claimed c JOIN problems p ON p.id = c.problem_id \
             JOIN hosts h ON h.id = p.host_id ORDER BY c.id",
        )
        .bind(now)
        .bind(now + LEASE)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(OutboxItem {
                    id: r.0,
                    recipient: r.1,
                    attempts: r.2,
                    created_at: r.3,
                    notification: Notification {
                        kind: parse_event(&r.4)?,
                        host: r.5,
                        trigger: r.6,
                        instance: r.7,
                        severity: parse_severity(&r.8)?,
                        summary: r.9,
                        started_at: r.10,
                        resolved_at: r.11,
                        resolve_reason: r.12,
                    },
                })
            })
            .collect()
    }

    pub async fn mark_notification_sent(
        &self,
        id: i64,
        now: OffsetDateTime,
    ) -> Result<(), StateError> {
        let done = sqlx::query(
            "UPDATE notification_outbox SET sent_at = $2, locked_until = NULL WHERE id = $1",
        )
        .bind(id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StateError::NotFound);
        }
        Ok(())
    }

    /// Keeps the row pending: clears the lease, stores the error and sets the next attempt.
    pub async fn mark_notification_retry(
        &self,
        id: i64,
        error: &str,
        next_attempt_at: OffsetDateTime,
    ) -> Result<(), StateError> {
        let done = sqlx::query(
            "UPDATE notification_outbox \
             SET locked_until = NULL, last_error = $2, next_attempt_at = $3 WHERE id = $1",
        )
        .bind(id)
        .bind(cleaned(error, MAX_ERROR_CHARS))
        .bind(next_attempt_at)
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StateError::NotFound);
        }
        Ok(())
    }

    /// Gives up on the row for good.
    pub async fn mark_notification_failed(
        &self,
        id: i64,
        error: &str,
        now: OffsetDateTime,
    ) -> Result<(), StateError> {
        let done = sqlx::query(
            "UPDATE notification_outbox \
             SET failed_at = $3, locked_until = NULL, last_error = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(cleaned(error, MAX_ERROR_CHARS))
        .bind(now)
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StateError::NotFound);
        }
        Ok(())
    }
}

/// Upserts the states in one statement; the last entry of a duplicated key wins.
async fn write_states(
    tx: &mut sqlx::PgConnection,
    host_id: Uuid,
    states: &[TriggerState],
) -> Result<(), StateError> {
    if states.is_empty() {
        return Ok(());
    }
    let unique: BTreeMap<(&str, &str), &TriggerState> = states
        .iter()
        .map(|s| ((s.trigger.as_str(), s.instance.as_str()), s))
        .collect();
    let mut triggers = Vec::with_capacity(unique.len());
    let mut instances = Vec::with_capacity(unique.len());
    let mut firing = Vec::with_capacity(unique.len());
    let mut since = Vec::with_capacity(unique.len());
    let mut last_ts = Vec::with_capacity(unique.len());
    let mut last_value = Vec::with_capacity(unique.len());
    for s in unique.values() {
        triggers.push(s.trigger.clone());
        instances.push(s.instance.clone());
        firing.push(s.firing);
        since.push(s.condition_since_ms.map(from_ms).transpose()?);
        last_ts.push(from_ms(s.last_ts_ms)?);
        last_value.push(s.last_value);
    }
    sqlx::query(
        "INSERT INTO trigger_states \
         (host_id, trigger_name, instance, firing, condition_since, last_ts, last_value) \
         SELECT $1, * FROM UNNEST($2::text[], $3::text[], $4::bool[], $5::timestamptz[], \
                                  $6::timestamptz[], $7::float8[]) \
         ON CONFLICT (host_id, trigger_name, instance) DO UPDATE SET \
             firing = EXCLUDED.firing, condition_since = EXCLUDED.condition_since, \
             last_ts = EXCLUDED.last_ts, last_value = EXCLUDED.last_value",
    )
    .bind(host_id)
    .bind(&triggers)
    .bind(&instances)
    .bind(&firing)
    .bind(&since)
    .bind(&last_ts)
    .bind(&last_value)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cerbrix_common::config::Severity;
    use sqlx::PgPool;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::*;
    use crate::alerting::{Evaluation, EventKind, OpenProblem, ResolveProblem, TriggerState};
    use crate::gitops::{compile, read_tree};
    use crate::state::{StateError, StateStore};

    const T0_MS: i64 = 1_800_000_000_000;

    fn at(ms: i64) -> OffsetDateTime {
        from_ms(ms).unwrap()
    }

    async fn setup(pool: PgPool) -> (StateStore, Uuid) {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        let compiled = compile(&read_tree(&dir, "c1").unwrap()).unwrap();
        let store = StateStore::from_pool(pool);
        store
            .apply_config("c1", &compiled, at(T0_MS), 50)
            .await
            .unwrap();
        let host = store.host_by_name("web-01").await.unwrap().unwrap();
        (store, host.id)
    }

    fn open(trigger: &str, severity: Severity, at_ms: i64) -> OpenProblem {
        OpenProblem {
            trigger: trigger.to_owned(),
            instance: String::new(),
            severity,
            at_ms,
            summary: format!("{trigger} is bad"),
        }
    }

    fn resolve(trigger: &str, at_ms: i64) -> ResolveProblem {
        ResolveProblem {
            trigger: trigger.to_owned(),
            instance: String::new(),
            at_ms,
        }
    }

    fn opening(trigger: &str, severity: Severity, at_ms: i64) -> Evaluation {
        Evaluation {
            open: vec![open(trigger, severity, at_ms)],
            ..Evaluation::default()
        }
    }

    fn resolving(trigger: &str, at_ms: i64) -> Evaluation {
        Evaluation {
            resolve: vec![resolve(trigger, at_ms)],
            ..Evaluation::default()
        }
    }

    async fn outbox(pool: &PgPool, event: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT recipient FROM notification_outbox WHERE event = $1 ORDER BY recipient",
        )
        .bind(event)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn open_creates_one_problem_and_event(pool: PgPool) {
        let (store, host) = setup(pool).await;
        let events = store
            .evaluate_host(host, false, |_| opening("Disk", Severity::High, T0_MS))
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Started);
        let problems = store.problems(ProblemFilter::Open, 10).await.unwrap();
        assert_eq!(problems.len(), 1);
        let p = &problems[0];
        assert_eq!(p.id, events[0].problem_id);
        assert_eq!(p.host, "web-01");
        assert_eq!(p.host_id, host);
        assert_eq!(p.trigger, "Disk");
        assert_eq!(p.severity, Severity::High);
        assert_eq!(p.summary, "Disk is bad");
        assert_eq!(p.started_at, at(T0_MS));
        assert_eq!(p.resolved_at, None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn open_twice_is_one_problem(pool: PgPool) {
        let (store, host) = setup(pool).await;
        let first = store
            .evaluate_host(host, true, |_| opening("Disk", Severity::High, T0_MS))
            .await
            .unwrap();
        let second = store
            .evaluate_host(host, true, |_| opening("Disk", Severity::High, T0_MS + 5))
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        assert!(second.is_empty());
        assert_eq!(
            store.problems(ProblemFilter::All, 10).await.unwrap().len(),
            1
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn resolve_closes_and_reports_once(pool: PgPool) {
        let (store, host) = setup(pool).await;
        // Nothing open: nothing to report.
        let none = store
            .evaluate_host(host, false, |_| resolving("Disk", T0_MS))
            .await
            .unwrap();
        assert!(none.is_empty());
        store
            .evaluate_host(host, false, |_| opening("Disk", Severity::High, T0_MS))
            .await
            .unwrap();
        let ended = store
            .evaluate_host(host, false, |_| resolving("Disk", T0_MS + 60_000))
            .await
            .unwrap();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].kind, EventKind::Resolved);
        let again = store
            .evaluate_host(host, false, |_| resolving("Disk", T0_MS + 90_000))
            .await
            .unwrap();
        assert!(again.is_empty());
        let all = store.problems(ProblemFilter::All, 10).await.unwrap();
        assert_eq!(all[0].resolved_at, Some(at(T0_MS + 60_000)));
        assert_eq!(all[0].resolve_reason.as_deref(), Some("condition_ended"));
        assert!(
            store
                .problems(ProblemFilter::Open, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn open_then_resolve_in_one_batch(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        let events = store
            .evaluate_host(host, true, |_| Evaluation {
                // Listed in the wrong order on purpose: time order decides.
                resolve: vec![resolve("Disk", T0_MS + 1000)],
                open: vec![open("Disk", Severity::Critical, T0_MS)],
                ..Evaluation::default()
            })
            .await
            .unwrap();
        let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [EventKind::Started, EventKind::Resolved]);
        let all = store.problems(ProblemFilter::All, 10).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].resolved_at, Some(at(T0_MS + 1000)));
        assert_eq!(outbox(&pool, "started").await, ["ops@example.com"]);
        assert_eq!(outbox(&pool, "resolved").await, ["ops@example.com"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn states_round_trip(pool: PgPool) {
        let (store, host) = setup(pool).await;
        let written = vec![
            TriggerState {
                trigger: "Disk".into(),
                instance: "mount=/".into(),
                firing: true,
                condition_since_ms: Some(T0_MS - 5000),
                last_ts_ms: T0_MS + 123,
                last_value: Some(91.5),
            },
            TriggerState {
                trigger: "Disk".into(),
                instance: "mount=/data".into(),
                firing: false,
                condition_since_ms: None,
                last_ts_ms: T0_MS,
                last_value: None,
            },
        ];
        let w = written.clone();
        store
            .evaluate_host(host, false, move |ctx| {
                assert!(ctx.states.is_empty());
                Evaluation {
                    states: w,
                    ..Evaluation::default()
                }
            })
            .await
            .unwrap();
        let mut seen = None;
        let updated = TriggerState {
            firing: false,
            last_ts_ms: T0_MS + 1,
            ..written[0].clone()
        };
        let u = updated.clone();
        store
            .evaluate_host(host, false, |ctx| {
                seen = Some(ctx.clone());
                Evaluation {
                    states: vec![u],
                    ..Evaluation::default()
                }
            })
            .await
            .unwrap();
        let ctx = seen.unwrap();
        assert_eq!(ctx.host, "web-01");
        assert_eq!(ctx.groups, ["production", "web"]);
        assert!(ctx.config.is_some());
        let mut sorted = ctx.states.clone();
        sorted.sort_by(|a, b| a.instance.cmp(&b.instance));
        assert_eq!(sorted, [written[0].clone(), written[1].clone()]);
        // The second evaluation replaced exactly the row it wrote.
        let mut after = None;
        store
            .evaluate_host(host, false, |ctx| {
                after = Some(ctx.states.clone());
                Evaluation::default()
            })
            .await
            .unwrap();
        let mut after = after.unwrap();
        after.sort_by(|a, b| a.instance.cmp(&b.instance));
        assert_eq!(after, [updated, written[1].clone()]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn inactive_host_is_not_evaluated(pool: PgPool) {
        let (store, host) = setup(pool).await;
        store.set_host_active(host, false).await.unwrap();
        let called = AtomicBool::new(false);
        let events = store
            .evaluate_host(host, true, |_| {
                called.store(true, Ordering::SeqCst);
                opening("Disk", Severity::High, T0_MS)
            })
            .await
            .unwrap();
        assert!(events.is_empty());
        assert!(!called.load(Ordering::SeqCst));
        let unknown = store
            .evaluate_host(Uuid::new_v4(), true, |_| {
                called.store(true, Ordering::SeqCst);
                Evaluation::default()
            })
            .await
            .unwrap();
        assert!(unknown.is_empty());
        assert!(!called.load(Ordering::SeqCst));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn started_queues_mail_for_matching_rules(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        store
            .evaluate_host(host, false, |_| opening("A", Severity::Critical, T0_MS))
            .await
            .unwrap();
        assert!(outbox(&pool, "started").await.is_empty());
        store
            .evaluate_host(host, true, |_| opening("B", Severity::Warning, T0_MS))
            .await
            .unwrap();
        assert!(outbox(&pool, "started").await.is_empty());
        store
            .evaluate_host(host, true, |_| opening("C", Severity::High, T0_MS + 7))
            .await
            .unwrap();
        assert_eq!(outbox(&pool, "started").await, ["ops@example.com"]);
        let (created, next): (OffsetDateTime, OffsetDateTime) =
            sqlx::query_as("SELECT created_at, next_attempt_at FROM notification_outbox")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((created, next), (at(T0_MS + 7), at(T0_MS + 7)));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn resolved_goes_to_those_who_got_started(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        store
            .evaluate_host(host, true, |_| opening("Disk", Severity::High, T0_MS))
            .await
            .unwrap();
        sqlx::query(
            "UPDATE config_documents \
             SET body = jsonb_set(body, '{email}', '[\"new@example.com\"]') \
             WHERE kind = 'notification'",
        )
        .execute(&pool)
        .await
        .unwrap();
        store
            .evaluate_host(host, true, |_| resolving("Disk", T0_MS + 1000))
            .await
            .unwrap();
        assert_eq!(outbox(&pool, "started").await, ["ops@example.com"]);
        assert_eq!(outbox(&pool, "resolved").await, ["ops@example.com"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn broken_rule_document_is_skipped(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        sqlx::query(
            "INSERT INTO config_documents (kind, id, source_file, body) \
             VALUES ('notification', 'broken', 'notifications/broken.json', '{\"nope\": 1}')",
        )
        .execute(&pool)
        .await
        .unwrap();
        store
            .evaluate_host(host, true, |_| opening("Disk", Severity::High, T0_MS))
            .await
            .unwrap();
        assert_eq!(outbox(&pool, "started").await, ["ops@example.com"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn concurrent_evaluations_of_one_host_serialise(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        let (a, b) = tokio::join!(
            store.evaluate_host(host, true, |_| opening("Disk", Severity::High, T0_MS)),
            store.evaluate_host(host, true, |_| opening("Disk", Severity::High, T0_MS)),
        );
        let total = a.unwrap().len() + b.unwrap().len();
        assert_eq!(total, 1);
        assert_eq!(
            store.problems(ProblemFilter::All, 10).await.unwrap().len(),
            1
        );
        assert_eq!(outbox(&pool, "started").await.len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn failure_rolls_back_the_whole_evaluation(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        // The first problem and its mail are written, then PostgreSQL rejects the NUL
        // character in the second summary: nothing may stay.
        let mut bad = open("B", Severity::High, T0_MS + 1);
        bad.summary = "a\0b".to_owned();
        let result = store
            .evaluate_host(host, true, |_| Evaluation {
                states: vec![TriggerState {
                    trigger: "A".into(),
                    instance: String::new(),
                    firing: true,
                    condition_since_ms: None,
                    last_ts_ms: T0_MS,
                    last_value: None,
                }],
                open: vec![open("A", Severity::High, T0_MS), bad],
                ..Evaluation::default()
            })
            .await;
        assert!(matches!(result, Err(StateError::Db(_))));
        assert!(
            store
                .problems(ProblemFilter::All, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(outbox(&pool, "started").await.is_empty());
        let states: i64 = sqlx::query_scalar("SELECT count(*) FROM trigger_states")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(states, 0);
    }

    async fn one_row_store(pool: PgPool) -> (StateStore, Uuid) {
        let (store, host) = setup(pool).await;
        store
            .evaluate_host(host, true, |_| opening("Disk", Severity::High, T0_MS))
            .await
            .unwrap();
        (store, host)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn claim_respects_due_time_lease_and_order(pool: PgPool) {
        let (store, host) = one_row_store(pool).await;
        store
            .evaluate_host(host, true, |_| {
                opening("Mem", Severity::Critical, T0_MS + 10)
            })
            .await
            .unwrap();
        // Not due yet.
        assert!(
            store
                .claim_notifications(at(T0_MS - 1), 10)
                .await
                .unwrap()
                .is_empty()
        );
        let first = store.claim_notifications(at(T0_MS), 10).await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].attempts, 1);
        assert_eq!(first[0].recipient, "ops@example.com");
        assert_eq!(first[0].notification.kind, EventKind::Started);
        assert_eq!(first[0].notification.host, "web-01");
        assert_eq!(first[0].notification.trigger, "Disk");
        let second = store.claim_notifications(at(T0_MS + 10), 10).await.unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].notification.trigger, "Mem");
        assert!(first[0].id < second[0].id);
        // Both leased: nothing before the lease of the first ends.
        let lease_end = at(T0_MS) + Duration::minutes(5);
        assert!(
            store
                .claim_notifications(lease_end - Duration::seconds(1), 10)
                .await
                .unwrap()
                .is_empty()
        );
        // After the lease ends the oldest row is claimed again and attempts count.
        let again = store
            .claim_notifications(lease_end + Duration::seconds(1), 1)
            .await
            .unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].id, first[0].id);
        assert_eq!(again[0].attempts, 2);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn resolved_waits_for_started(pool: PgPool) {
        for fail_started in [false, true] {
            let (store, host) = one_row_store(pool.clone()).await;
            store
                .evaluate_host(host, true, |_| resolving("Disk", T0_MS + 1000))
                .await
                .unwrap();
            let now = at(T0_MS + 2000);
            let first = store.claim_notifications(now, 10).await.unwrap();
            assert_eq!(first.len(), 1);
            assert_eq!(first[0].notification.kind, EventKind::Started);
            // Still in flight: the resolved row keeps waiting.
            assert!(store.claim_notifications(now, 10).await.unwrap().is_empty());
            if fail_started {
                store
                    .mark_notification_failed(first[0].id, "boom", now)
                    .await
                    .unwrap();
            } else {
                store
                    .mark_notification_sent(first[0].id, now)
                    .await
                    .unwrap();
            }
            let next = store.claim_notifications(now, 10).await.unwrap();
            assert_eq!(next.len(), 1);
            assert_eq!(next[0].notification.kind, EventKind::Resolved);
            assert_eq!(next[0].notification.resolved_at, Some(at(T0_MS + 1000)));
            assert_eq!(
                next[0].notification.resolve_reason.as_deref(),
                Some("condition_ended")
            );
            sqlx::query("DELETE FROM problems")
                .execute(&pool)
                .await
                .unwrap();
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn retry_clears_lease_and_cuts_error(pool: PgPool) {
        let (store, _) = one_row_store(pool.clone()).await;
        let now = at(T0_MS);
        let item = store.claim_notifications(now, 1).await.unwrap().remove(0);
        let long = format!("bad\u{7}\n{}", "x".repeat(900));
        let next = now + Duration::minutes(1);
        store
            .mark_notification_retry(item.id, &long, next)
            .await
            .unwrap();
        let (error, locked, due): (String, Option<OffsetDateTime>, OffsetDateTime) =
            sqlx::query_as(
                "SELECT last_error, locked_until, next_attempt_at \
                 FROM notification_outbox WHERE id = $1",
            )
            .bind(item.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(error.chars().count(), 500);
        assert!(error.starts_with("badx"));
        assert_eq!(locked, None);
        assert_eq!(due, next);
        assert!(store.claim_notifications(now, 1).await.unwrap().is_empty());
        assert_eq!(store.claim_notifications(next, 1).await.unwrap().len(), 1);
        assert!(matches!(
            store.mark_notification_sent(i64::MAX, now).await,
            Err(StateError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn two_claimers_get_disjoint_rows(pool: PgPool) {
        let (store, host) = one_row_store(pool.clone()).await;
        store
            .evaluate_host(host, true, |_| opening("Mem", Severity::High, T0_MS))
            .await
            .unwrap();
        let now = at(T0_MS);
        for _ in 0..5 {
            // Release the leases so every round claims the same two rows.
            sqlx::query("UPDATE notification_outbox SET locked_until = NULL")
                .execute(&pool)
                .await
                .unwrap();
            let (a, b) = tokio::join!(
                store.claim_notifications(now, 1),
                store.claim_notifications(now, 1)
            );
            let mut ids: Vec<i64> = a
                .unwrap()
                .iter()
                .chain(b.unwrap().iter())
                .map(|i| i.id)
                .collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), 2);
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn acknowledge_sets_fields_and_unknown_id_is_not_found(pool: PgPool) {
        let (store, host) = setup(pool).await;
        let events = store
            .evaluate_host(host, false, |_| opening("Disk", Severity::High, T0_MS))
            .await
            .unwrap();
        let id = events[0].problem_id;
        let p = store
            .acknowledge_problem(id, "anna", "looking", at(T0_MS + 5))
            .await
            .unwrap();
        assert_eq!(p.acknowledged_by.as_deref(), Some("anna"));
        assert_eq!(p.ack_comment.as_deref(), Some("looking"));
        assert_eq!(p.acknowledged_at, Some(at(T0_MS + 5)));
        assert_eq!(p.host, "web-01");
        let long_by = "b".repeat(300);
        let long_comment = "c".repeat(1500);
        let p = store
            .acknowledge_problem(id, &long_by, &long_comment, at(T0_MS + 9))
            .await
            .unwrap();
        assert_eq!(p.acknowledged_by.unwrap().chars().count(), 200);
        assert_eq!(p.ack_comment.unwrap().chars().count(), 1000);
        assert_eq!(p.acknowledged_at, Some(at(T0_MS + 9)));
        assert!(matches!(
            store
                .acknowledge_problem(Uuid::new_v4(), "x", "y", at(T0_MS))
                .await,
            Err(StateError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn problems_filter_and_order(pool: PgPool) {
        let (store, host) = setup(pool).await;
        let eval = Evaluation {
            open: vec![
                open("warn-old", Severity::Warning, T0_MS),
                open("crit-new", Severity::Critical, T0_MS + 3000),
                open("crit-old", Severity::Critical, T0_MS + 1000),
                open("high", Severity::High, T0_MS + 500),
                open("info", Severity::Info, T0_MS - 1000),
                open("done", Severity::Critical, T0_MS + 4000),
            ],
            resolve: vec![resolve("done", T0_MS + 5000)],
            ..Evaluation::default()
        };
        store.evaluate_host(host, false, |_| eval).await.unwrap();
        let names = |v: Vec<Problem>| v.into_iter().map(|p| p.trigger).collect::<Vec<_>>();
        assert_eq!(
            names(store.problems(ProblemFilter::Open, 10).await.unwrap()),
            ["crit-old", "crit-new", "high", "warn-old", "info"]
        );
        assert_eq!(
            names(store.problems(ProblemFilter::All, 10).await.unwrap()),
            ["done", "crit-new", "crit-old", "high", "warn-old", "info"]
        );
        assert_eq!(
            store.problems(ProblemFilter::All, 2).await.unwrap().len(),
            2
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn touch_seen_upserts_and_never_moves_back(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        let mount = |m: &str| Params::from([("mount".to_owned(), m.to_owned())]);
        let series = vec![
            ("disk.used_percent".to_owned(), mount("/")),
            ("disk.used_percent".to_owned(), mount("/")),
            ("cpu.usage".to_owned(), Params::new()),
        ];
        store.touch_seen(host, at(T0_MS), &series).await.unwrap();
        store
            .touch_seen(host, at(T0_MS - 5000), &series[..1])
            .await
            .unwrap();
        let rows: Vec<(String, String, serde_json::Value, OffsetDateTime)> = sqlx::query_as(
            "SELECT item_key, instance, params, last_arrival FROM series_seen \
             ORDER BY item_key",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            [
                (
                    "cpu.usage".to_owned(),
                    String::new(),
                    serde_json::json!({}),
                    at(T0_MS)
                ),
                (
                    "disk.used_percent".to_owned(),
                    "mount=/".to_owned(),
                    serde_json::json!({"mount": "/"}),
                    at(T0_MS)
                ),
            ]
        );
        let seen: Option<OffsetDateTime> =
            sqlx::query_scalar("SELECT last_seen_at FROM hosts WHERE id = $1")
                .bind(host)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(seen, Some(at(T0_MS)));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn heartbeat_sets_baseline_after_a_gap(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let every = std::time::Duration::from_secs(15);
        let t = at(T0_MS);
        // The first call has no previous heartbeat: a gap.
        assert_eq!(store.alerting_heartbeat(t, every).await.unwrap(), Some(t));
        // 15 s later: no gap, the baseline stays.
        assert_eq!(
            store
                .alerting_heartbeat(t + Duration::seconds(15), every)
                .await
                .unwrap(),
            Some(t)
        );
        // 10 minutes after that: a gap, the baseline moves to now.
        let later = t + Duration::seconds(15) + Duration::minutes(10);
        assert_eq!(
            store.alerting_heartbeat(later, every).await.unwrap(),
            Some(later)
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn hosts_with_nodata_triggers_lists_only_active_hosts_with_such_triggers(pool: PgPool) {
        use crate::gitops::Snapshot;
        let host = |name: &str, trigger: &str| {
            format!(
                r#"{{"host":"{name}","items":[{{"key":"cpu.usage","interval":"1m"}}],
                    "triggers":[{trigger}]}}"#
            )
        };
        let silent = r#"{"name":"Silent","expression":"no data for 3m","severity":"high"}"#;
        let cpu = r#"{"name":"Cpu","expression":"cpu.usage > 90","severity":"high"}"#;
        let files = [
            ("hosts/a.json", host("a-silent", silent)),
            ("hosts/b.json", host("b-compare", cpu)),
            ("hosts/c.json", host("c-inactive", silent)),
            ("hosts/d.json", r#"{"host":"d-nothing"}"#.to_owned()),
        ];
        let snapshot = Snapshot {
            commit: "c1".into(),
            files: files
                .into_iter()
                .map(|(p, c)| (p.to_owned(), c.into_bytes()))
                .collect(),
        };
        let compiled = compile(&snapshot).unwrap_or_else(|i| panic!("{i:?}"));
        let store = StateStore::from_pool(pool.clone());
        store
            .apply_config("c1", &compiled, at(T0_MS), 50)
            .await
            .unwrap();
        sqlx::query("UPDATE hosts SET active = false WHERE name = 'c-inactive'")
            .execute(&pool)
            .await
            .unwrap();
        let a = store.host_by_name("a-silent").await.unwrap().unwrap().id;
        assert_eq!(store.hosts_with_nodata_triggers().await.unwrap(), [a]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn seen_returns_host_and_series_arrivals(pool: PgPool) {
        let (store, host) = setup(pool).await;
        assert_eq!(store.seen(host).await.unwrap(), Seen::default());
        let mount = Params::from([("mount".to_owned(), "/".to_owned())]);
        store
            .touch_seen(
                host,
                at(T0_MS),
                &[("disk.used_percent".to_owned(), mount.clone())],
            )
            .await
            .unwrap();
        assert_eq!(
            store.seen(host).await.unwrap(),
            Seen {
                last_seen_ms: Some(T0_MS),
                series: vec![("disk.used_percent".to_owned(), mount, T0_MS)],
            }
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn migration_leaves_last_seen_null(pool: PgPool) {
        let (_store, host) = setup(pool.clone()).await;
        let seen: Option<OffsetDateTime> =
            sqlx::query_scalar("SELECT last_seen_at FROM hosts WHERE id = $1")
                .bind(host)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(seen, None);
    }
}
