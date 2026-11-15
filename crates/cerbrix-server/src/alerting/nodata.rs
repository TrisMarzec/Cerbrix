//! "No data" triggers: a whole host, or one item of a host, stopped reporting.
//!
//! Silence is measured from the time values last arrived at the server, never from
//! their measurement time. A gap in the server's own heartbeat is not silence of the
//! hosts: after such a gap every host counts as having reported at the baseline.

use std::sync::Arc;
use std::time::Duration;

use cerbrix_common::{Interval, Selector, TriggerExpr};
use time::OffsetDateTime;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::{
    ActiveTrigger, Evaluation, OpenProblem, Params, ResolveProblem, TriggerState, active_triggers,
    instance_key,
};
use crate::state::{StateError, StateStore};

/// When values last arrived for one host, as stored by `touch_seen`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    /// Arrival of the host's latest value; `None` if it never sent one.
    pub last_seen_ms: Option<i64>,
    /// Item key, parameter map and arrival of the latest value of every watched series.
    pub series: Vec<(String, Params, i64)>,
}

fn window_ms(window: Duration) -> i64 {
    i64::try_from(window.as_millis()).unwrap_or(i64::MAX)
}

/// The window as the interval type prints it: `3m`, `90s`, `1h`.
fn window_text(window: Duration) -> String {
    let secs = window.as_secs();
    format!("{secs}s")
        .parse::<Interval>()
        .map_or_else(|_| format!("{secs}s"), |i| i.to_string())
}

fn matches(selector: &Selector, key: &str, params: &Params) -> bool {
    selector.key == key
        && selector
            .params
            .iter()
            .all(|(k, want)| params.get(k) == Some(want))
}

/// Decides which no-data problems open and which resolve.
///
/// Only `TriggerExpr::NoData` triggers are evaluated. A host or series counts as silent
/// since the later of its last arrival and `baseline_ms` (the end of the server's own last
/// downtime). An item-level trigger fires only while the host as a whole still reports; when
/// the host is silent only the host-level trigger fires, and an item problem that is already
/// open stays as it is. A host that never sent a value produces nothing. The result is
/// ordered by trigger name, then instance. The function never panics; all time arithmetic
/// saturates.
pub fn evaluate_nodata(
    triggers: &[ActiveTrigger],
    states: &[TriggerState],
    seen: &Seen,
    baseline_ms: Option<i64>,
    now_ms: i64,
) -> Evaluation {
    let mut out = Evaluation::default();
    let Some(last_seen) = seen.last_seen_ms else {
        return out;
    };
    let baseline = baseline_ms.unwrap_or(i64::MIN);
    let host_since = last_seen.max(baseline);

    let is_firing = |trigger: &str, instance: &str| {
        states
            .iter()
            .any(|s| s.firing && s.trigger == trigger && s.instance == instance)
    };
    let state = |trigger: &str, instance: &str, firing: bool| TriggerState {
        trigger: trigger.to_owned(),
        instance: instance.to_owned(),
        firing,
        condition_since_ms: None,
        last_ts_ms: now_ms,
        last_value: None,
    };

    let mut sorted: Vec<&ActiveTrigger> = triggers.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for t in sorted {
        let TriggerExpr::NoData { selector, window } = &t.expr else {
            continue;
        };
        let window_ms = window_ms(*window);
        let silent = |since: i64| now_ms.saturating_sub(since) >= window_ms;
        let host_silent = silent(host_since);

        // Host level: one instance, "".
        let Some(selector) = selector else {
            let summary = format!("no data for {}", window_text(*window));
            apply(
                &mut out,
                t,
                "",
                host_silent,
                true,
                &summary,
                now_ms,
                &is_firing,
                &state,
            );
            continue;
        };

        let mut series: Vec<(String, i64)> = seen
            .series
            .iter()
            .filter(|(key, params, _)| matches(selector, key, params))
            .map(|(_, params, at)| (instance_key(params), (*at).max(baseline)))
            .collect();
        series.sort();
        for (instance, since) in series {
            let mut summary = format!("{}: no data for {}", selector.key, window_text(*window));
            if !instance.is_empty() {
                summary.push_str(" on ");
                summary.push_str(&instance);
            }
            // While the host as a whole is silent the host-level trigger speaks for it:
            // no item problem opens, and one that is open stays.
            let item_silent = silent(since);
            let fires = item_silent && !host_silent;
            let resolves = !item_silent && !host_silent;
            apply(
                &mut out, t, &instance, fires, resolves, &summary, now_ms, &is_firing, &state,
            );
        }
    }
    out
}

/// Opens when `fires` and not yet firing; resolves when firing and `resolves`.
#[allow(clippy::too_many_arguments)]
fn apply(
    out: &mut Evaluation,
    t: &ActiveTrigger,
    instance: &str,
    fires: bool,
    resolves: bool,
    summary: &str,
    now_ms: i64,
    is_firing: &dyn Fn(&str, &str) -> bool,
    state: &dyn Fn(&str, &str, bool) -> TriggerState,
) {
    let firing = is_firing(&t.name, instance);
    if fires && !firing {
        out.states.push(state(&t.name, instance, true));
        out.open.push(OpenProblem {
            trigger: t.name.clone(),
            instance: instance.to_owned(),
            severity: t.severity,
            at_ms: now_ms,
            summary: summary.to_owned(),
        });
    } else if resolves && firing && !fires {
        out.states.push(state(&t.name, instance, false));
        out.resolve.push(ResolveProblem {
            trigger: t.name.clone(),
            instance: instance.to_owned(),
            at_ms: now_ms,
        });
    }
}

fn to_ms(t: OffsetDateTime) -> i64 {
    i64::try_from(t.unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(i64::MAX)
}

/// One no-data check of every host that has a no-data trigger; returns how many hosts
/// were checked.
///
/// Records the heartbeat first (which moves the baseline after a gap in the server's own
/// operation), then evaluates each host in its own transaction. A host that fails is logged
/// and skipped; only a failing heartbeat or host listing is an error. Safe to run on
/// several instances at once: `evaluate_host` locks the host and an open problem is unique.
pub async fn check_once(
    store: &StateStore,
    queue_mail: bool,
    check_every: Duration,
    now: OffsetDateTime,
) -> Result<usize, StateError> {
    let baseline_ms = store.alerting_heartbeat(now, check_every).await?.map(to_ms);
    let now_ms = to_ms(now);
    let hosts = store.hosts_with_nodata_triggers().await?;
    let mut checked = 0;
    for host_id in hosts {
        // Read right before the evaluation; a value arriving in between is picked up by
        // the next check.
        let seen = match store.seen(host_id).await {
            Ok(seen) => seen,
            Err(err) => {
                tracing::error!(%host_id, %err, "no-data check: cannot read when values arrived");
                continue;
            }
        };
        let result = store
            .evaluate_host(host_id, queue_mail, move |ctx| {
                let Some(config) = &ctx.config else {
                    return Evaluation::default();
                };
                let triggers = active_triggers(config);
                evaluate_nodata(&triggers, &ctx.states, &seen, baseline_ms, now_ms)
            })
            .await;
        match result {
            Ok(events) => {
                checked += 1;
                if !events.is_empty() {
                    tracing::info!(%host_id, events = events.len(), "no-data check changed problems");
                }
            }
            Err(err) => tracing::error!(%host_id, %err, "no-data check failed for a host"),
        }
    }
    Ok(checked)
}

/// Runs a check immediately and then once per `check_every` (measured from the start of the
/// previous check) until `shutdown` is cancelled.
pub async fn run_nodata_loop(
    store: StateStore,
    queue_mail: bool,
    check_every: Duration,
    shutdown: CancellationToken,
) {
    let mut ticker = tokio::time::interval(check_every.max(Duration::from_millis(1)));
    // A check that overruns the interval delays the next one instead of causing a burst.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let store = Arc::new(store);

    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = ticker.tick() => {}
        }

        // A separate task, so that a panic in one check is caught and logged.
        let mut task = tokio::spawn({
            let store = Arc::clone(&store);
            async move { check_once(&store, queue_mail, check_every, OffsetDateTime::now_utc()).await }
        });
        let joined = tokio::select! {
            () = shutdown.cancelled() => {
                // Wait until the aborted check is gone, so that its transaction is
                // dropped (rolled back) before the loop returns.
                task.abort();
                let _ = task.await;
                break;
            }
            joined = &mut task => joined,
        };
        match joined {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => tracing::error!(%err, "no-data check failed"),
            Err(err) => tracing::error!(%err, "no-data check panicked"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerting::{ActiveTrigger, Params, TriggerState};
    use cerbrix_common::config::Severity;
    use cerbrix_common::parse_trigger;

    const NOW: i64 = 1_000_000_000;
    const MIN: i64 = 60_000;

    fn trig(name: &str, expr: &str, severity: Severity) -> ActiveTrigger {
        ActiveTrigger {
            name: name.into(),
            severity,
            expression: expr.into(),
            expr: parse_trigger(expr).unwrap(),
        }
    }

    fn host_trigger() -> ActiveTrigger {
        trig("Host silent", "no data for 3m", Severity::Critical)
    }

    fn disk_trigger() -> ActiveTrigger {
        trig(
            "Disk silent",
            "disk.used_percent no data for 5m",
            Severity::Warning,
        )
    }

    fn cpu_trigger() -> ActiveTrigger {
        trig("Cpu silent", "cpu.usage no data for 5m", Severity::High)
    }

    fn mount(m: &str) -> Params {
        Params::from([("mount".to_owned(), m.to_owned())])
    }

    fn disk(m: &str, ago_ms: i64) -> (String, Params, i64) {
        ("disk.used_percent".into(), mount(m), NOW - ago_ms)
    }

    fn seen(host_ago_ms: i64, series: Vec<(String, Params, i64)>) -> Seen {
        Seen {
            last_seen_ms: Some(NOW - host_ago_ms),
            series,
        }
    }

    fn firing(trigger: &str, instance: &str) -> TriggerState {
        TriggerState {
            trigger: trigger.into(),
            instance: instance.into(),
            firing: true,
            condition_since_ms: None,
            last_ts_ms: NOW - MIN,
            last_value: None,
        }
    }

    fn run(
        triggers: &[ActiveTrigger],
        states: &[TriggerState],
        seen: &Seen,
        baseline: Option<i64>,
    ) -> Evaluation {
        evaluate_nodata(triggers, states, seen, baseline, NOW)
    }

    #[test]
    fn host_silent_fires_after_window() {
        let t = [host_trigger()];
        let e = run(&t, &[], &seen(3 * MIN, vec![]), None);
        assert_eq!(e.open.len(), 1);
        let o = &e.open[0];
        assert_eq!(o.trigger, "Host silent");
        assert_eq!(o.instance, "");
        assert_eq!(o.severity, Severity::Critical);
        assert_eq!(o.at_ms, NOW);
        assert_eq!(o.summary, "no data for 3m");
        assert!(e.resolve.is_empty());
        assert_eq!(
            e.states,
            [TriggerState {
                trigger: "Host silent".into(),
                instance: String::new(),
                firing: true,
                condition_since_ms: None,
                last_ts_ms: NOW,
                last_value: None,
            }]
        );

        let e = run(&t, &[], &seen(3 * MIN - 1000, vec![]), None);
        assert_eq!(e, Evaluation::default());
    }

    #[test]
    fn host_never_seen_does_not_fire() {
        let t = [host_trigger(), disk_trigger()];
        let s = Seen::default();
        assert_eq!(run(&t, &[], &s, None), Evaluation::default());
        assert_eq!(
            run(&t, &[], &s, Some(NOW - 60 * MIN)),
            Evaluation::default()
        );
    }

    #[test]
    fn host_reporting_again_resolves() {
        let t = [host_trigger()];
        let st = [firing("Host silent", "")];
        let e = run(&t, &st, &seen(MIN, vec![]), None);
        assert!(e.open.is_empty());
        assert_eq!(e.resolve.len(), 1);
        assert_eq!(e.resolve[0].trigger, "Host silent");
        assert_eq!(e.resolve[0].at_ms, NOW);
        assert_eq!(e.states.len(), 1);
        assert!(!e.states[0].firing);
        // Still silent: nothing changes.
        let e = run(&t, &st, &seen(10 * MIN, vec![]), None);
        assert_eq!(e, Evaluation::default());
    }

    #[test]
    fn baseline_postpones_silence() {
        let t = [host_trigger()];
        let s = seen(10 * MIN, vec![]);
        assert_eq!(run(&t, &[], &s, Some(NOW - MIN)), Evaluation::default());
        assert_eq!(run(&t, &[], &s, Some(NOW - 4 * MIN)).open.len(), 1);
    }

    #[test]
    fn item_silent_while_host_reports_fires_per_instance() {
        let t = [disk_trigger()];
        let s = seen(MIN / 2, vec![disk("/", 6 * MIN), disk("/data", MIN)]);
        let e = run(&t, &[], &s, None);
        assert_eq!(e.open.len(), 1);
        let o = &e.open[0];
        assert_eq!(o.trigger, "Disk silent");
        assert_eq!(o.instance, "mount=/");
        assert_eq!(o.severity, Severity::Warning);
        assert_eq!(o.summary, "disk.used_percent: no data for 5m on mount=/");
        assert_eq!(e.states.len(), 1);
        assert_eq!(e.states[0].instance, "mount=/");
    }

    #[test]
    fn item_summary_without_instance_and_selector_params() {
        let t = [
            cpu_trigger(),
            trig(
                "Root silent",
                "disk.used_percent{mount=/} no data for 90s",
                Severity::Info,
            ),
        ];
        let s = seen(
            0,
            vec![
                ("cpu.usage".into(), Params::new(), NOW - 6 * MIN),
                disk("/", 2 * MIN),
                disk("/data", 10 * MIN),
            ],
        );
        let e = run(&t, &[], &s, None);
        let summaries: Vec<_> = e.open.iter().map(|o| o.summary.as_str()).collect();
        assert_eq!(
            summaries,
            [
                "cpu.usage: no data for 5m",
                "disk.used_percent: no data for 90s on mount=/"
            ]
        );
    }

    #[test]
    fn agent_outage_is_one_problem() {
        let t = [host_trigger(), disk_trigger(), cpu_trigger()];
        let s = seen(
            10 * MIN,
            vec![
                disk("/", 10 * MIN),
                ("cpu.usage".into(), Params::new(), NOW - 10 * MIN),
            ],
        );
        let e = run(&t, &[], &s, None);
        assert_eq!(e.open.len(), 1);
        assert_eq!(e.open[0].trigger, "Host silent");
    }

    #[test]
    fn firing_item_trigger_survives_host_outage() {
        let t = [host_trigger(), disk_trigger()];
        let st = [firing("Disk silent", "mount=/")];
        let s = seen(10 * MIN, vec![disk("/", 10 * MIN)]);
        let e = run(&t, &st, &s, None);
        assert!(e.resolve.is_empty());
        assert_eq!(e.open.len(), 1);
        assert_eq!(e.open[0].trigger, "Host silent");
    }

    #[test]
    fn item_reporting_again_resolves() {
        let t = [host_trigger(), disk_trigger()];
        let st = [firing("Disk silent", "mount=/")];
        let s = seen(0, vec![disk("/", 0)]);
        let e = run(&t, &st, &s, None);
        assert!(e.open.is_empty());
        assert_eq!(e.resolve.len(), 1);
        assert_eq!(e.resolve[0].trigger, "Disk silent");
        assert_eq!(e.resolve[0].instance, "mount=/");
        assert_eq!(e.resolve[0].at_ms, NOW);
        assert!(!e.states[0].firing);
    }

    #[test]
    fn no_second_open_while_firing() {
        let t = [host_trigger(), disk_trigger()];
        let st = [firing("Host silent", ""), firing("Disk silent", "mount=/")];
        // The host reports, the disk stays silent: the disk problem stays, nothing opens.
        let s = seen(0, vec![disk("/", 10 * MIN)]);
        let e = run(&t, &st[1..], &s, None);
        assert_eq!(e, Evaluation::default());
        // The host stays silent: its problem stays, nothing opens.
        let s = seen(10 * MIN, vec![disk("/", 10 * MIN)]);
        let e = run(&t, &st[..1], &s, None);
        assert_eq!(e, Evaluation::default());
    }

    #[test]
    fn compare_triggers_are_ignored_and_time_arithmetic_saturates() {
        let t = [
            trig("Cpu high", "cpu.usage > 90", Severity::High),
            trig("Never", "no data for 24h", Severity::High),
        ];
        let e = run(&t, &[], &seen(10 * MIN, vec![]), None);
        assert_eq!(e, Evaluation::default());
        let s = Seen {
            last_seen_ms: Some(i64::MIN),
            series: vec![("cpu.usage".into(), Params::new(), i64::MIN)],
        };
        let t = [host_trigger(), cpu_trigger()];
        let _ = evaluate_nodata(&t, &[], &s, Some(i64::MAX), i64::MIN);
        let _ = evaluate_nodata(&t, &[], &s, None, i64::MAX);
    }

    // ---- check_once and the loop, against the database ----

    use crate::gitops::{compile, read_tree};
    use crate::state::ProblemFilter;
    use sqlx::PgPool;
    use uuid::Uuid;

    const EVERY: Duration = Duration::from_secs(120);

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn after(secs: i64) -> OffsetDateTime {
        t0() + time::Duration::seconds(secs)
    }

    /// The example repository: `web-01` has the host-level trigger "Agent silent" (3m).
    async fn setup(pool: PgPool) -> (StateStore, Uuid) {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        let compiled = compile(&read_tree(&dir, "c1").unwrap()).unwrap();
        let store = StateStore::from_pool(pool);
        store.apply_config("c1", &compiled, t0(), 50).await.unwrap();
        let host = store.host_by_name("web-01").await.unwrap().unwrap();
        (store, host.id)
    }

    async fn set_heartbeat(pool: &PgPool, at: OffsetDateTime) {
        sqlx::query("UPDATE alerting_state SET heartbeat_at = $1, baseline_at = NULL")
            .bind(at)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn open_problems(store: &StateStore) -> Vec<(String, String)> {
        store
            .problems(ProblemFilter::Open, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|p| (p.host, p.trigger))
            .collect()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn check_once_opens_and_resolves_end_to_end(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        // web-01 last reported 5 minutes ago; db-01 never did. The check ran a minute ago.
        store.touch_seen(host, after(-300), &[]).await.unwrap();
        set_heartbeat(&pool, after(-60)).await;

        let checked = check_once(&store, false, EVERY, t0()).await.unwrap();
        assert_eq!(checked, 2);
        assert_eq!(
            open_problems(&store).await,
            [("web-01".to_owned(), "Agent silent".to_owned())]
        );
        let p = &store.problems(ProblemFilter::Open, 10).await.unwrap()[0];
        assert_eq!(p.summary, "no data for 3m");
        assert_eq!(p.started_at, t0());

        // The same silence again: still one problem.
        check_once(&store, false, EVERY, after(30)).await.unwrap();
        assert_eq!(open_problems(&store).await.len(), 1);

        store.touch_seen(host, after(40), &[]).await.unwrap();
        check_once(&store, false, EVERY, after(60)).await.unwrap();
        assert!(open_problems(&store).await.is_empty());
        let all = store.problems(ProblemFilter::All, 10).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].resolved_at, Some(after(60)));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn server_gap_does_not_alarm(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        store.touch_seen(host, after(-600), &[]).await.unwrap();
        set_heartbeat(&pool, after(-600)).await;

        // The server was down for 10 minutes: the host is not silent yet.
        check_once(&store, false, EVERY, t0()).await.unwrap();
        assert!(open_problems(&store).await.is_empty());

        // Four minutes later, still without a value: now it is.
        check_once(&store, false, EVERY, after(240)).await.unwrap();
        assert_eq!(
            open_problems(&store).await,
            [("web-01".to_owned(), "Agent silent".to_owned())]
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unreadable_host_configuration_is_skipped(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        store.touch_seen(host, after(-300), &[]).await.unwrap();
        set_heartbeat(&pool, after(-60)).await;
        // db-01 has a stored configuration that cannot be read: it is skipped.
        let db = store.host_by_name("db-01").await.unwrap().unwrap();
        sqlx::query("UPDATE hosts SET config = '[]'::jsonb WHERE id = $1")
            .bind(db.id)
            .execute(&pool)
            .await
            .unwrap();
        let checked = check_once(&store, false, EVERY, t0()).await.unwrap();
        assert_eq!(checked, 1);
        assert_eq!(open_problems(&store).await.len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn loop_stops_on_shutdown(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        // Silent for an hour, and a recent heartbeat: the first check opens a problem.
        let now = OffsetDateTime::now_utc();
        store
            .touch_seen(host, now - time::Duration::hours(1), &[])
            .await
            .unwrap();
        set_heartbeat(&pool, now).await;

        let shutdown = CancellationToken::new();
        // An hour between checks: only the immediate first check runs in this test.
        let handle = tokio::spawn(run_nodata_loop(
            store.clone(),
            false,
            Duration::from_secs(3600),
            shutdown.clone(),
        ));
        let mut opened = false;
        for _ in 0..600 {
            if !open_problems(&store).await.is_empty() {
                opened = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(30), handle)
            .await
            .expect("loop stops after shutdown")
            .expect("the loop itself did not panic");
        assert!(opened, "the first check opened the problem");
    }
}
