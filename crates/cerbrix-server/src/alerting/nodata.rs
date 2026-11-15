//! "No data" triggers: a whole host, or one item of a host, stopped reporting.
//!
//! Silence is measured from the time values last arrived at the server, never from
//! their measurement time. A gap in the server's own heartbeat is not silence of the
//! hosts: after such a gap every host counts as having reported at the baseline.

use std::sync::Arc;
use std::time::Duration;

use cerbrix_common::{Interval, Selector, TriggerExpr, parse_trigger};
use time::OffsetDateTime;
use tokio::task::JoinError;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::{
    ActiveTrigger, Evaluation, OpenProblem, Params, ResolveProblem, TriggerState, active_triggers,
    instance_key,
};
use crate::gitops::EffectiveHost;
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

/// Whether the series `key` with `params` is one that `selector` watches: the key is equal
/// and every parameter of the selector is present with the same value.
pub(crate) fn selector_matches(selector: &Selector, key: &str, params: &Params) -> bool {
    selector.key == key
        && selector
            .params
            .iter()
            .all(|(k, want)| params.get(k) == Some(want))
}

/// The selectors of the host's item-level no-data triggers: the series whose arrival is
/// tracked in `series_seen`. A trigger that does not parse watches nothing.
pub(crate) fn item_nodata_selectors(host: &EffectiveHost) -> Vec<Selector> {
    host.triggers
        .iter()
        .filter_map(|t| match parse_trigger(&t.expression) {
            Ok(TriggerExpr::NoData {
                selector: Some(selector),
                ..
            }) => Some(selector),
            _ => None,
        })
        .collect()
}

/// Decides which no-data problems open and which resolve.
///
/// Only `TriggerExpr::NoData` triggers are evaluated. A host or series counts as silent
/// since the later of its last arrival and `baseline_ms` (the end of the server's own last
/// downtime); the baseline only postpones opening, resolving looks at the real arrival. An
/// item-level trigger fires only for a series that was already silent for its window when the
/// host last reported, and only while the host as a whole is not silent: an agent outage, in
/// which everything stops at once, never yields item problems, whatever the windows. An item
/// problem that is already open stays as it is while the host is silent. A host that never
/// sent a value produces nothing. The result is ordered by trigger name, then instance. The
/// function never panics; all time arithmetic saturates.
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

        // Host level: one instance, "". The baseline only postpones opening; resolving
        // looks at the real last arrival.
        let Some(selector) = selector else {
            let summary = format!("no data for {}", window_text(*window));
            apply(
                &mut out,
                t,
                "",
                host_silent,
                !silent(last_seen),
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
            .filter(|(key, params, _)| selector_matches(selector, key, params))
            .map(|(_, params, at)| (instance_key(params), *at))
            .collect();
        series.sort();
        for (instance, arrived) in series {
            let mut summary = format!("{}: no data for {}", selector.key, window_text(*window));
            if !instance.is_empty() {
                summary.push_str(" on ");
                summary.push_str(&instance);
            }
            // "The host reports, but not this item": the series was already silent for the
            // window when the host last reported. During an agent outage `last_seen` stays
            // where it was, so the outage alone can never make an item trigger fire, whatever
            // its window compared with the host-level one. While the host as a whole is
            // silent the host-level trigger speaks for it: no item problem opens, and one that
            // is open stays.
            let item_since = arrived.max(baseline);
            let item_silent = silent(item_since);
            let silent_while_host_reported = last_seen.saturating_sub(item_since) >= window_ms;
            let fires = item_silent && silent_while_host_reported && !host_silent;
            // The baseline only postpones opening: an open problem resolves only when a
            // value really arrived again.
            let resolves = !silent(arrived) && !silent(last_seen);
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

/// How many hosts a check evaluates between two heartbeat refreshes.
const HEARTBEAT_EVERY_HOSTS: usize = 100;

/// One no-data check of every host that has a no-data trigger; returns how many hosts
/// were checked. [`begin_check`] followed by [`check_hosts`].
pub async fn check_once(
    store: &StateStore,
    queue_mail: bool,
    check_every: Duration,
    first_cycle: bool,
    now: OffsetDateTime,
) -> Result<usize, StateError> {
    let baseline_ms = begin_check(store, check_every, first_cycle, now).await?;
    check_hosts(store, queue_mail, baseline_ms, now).await
}

/// The first step of a check: records the heartbeat, which moves the baseline after a gap in
/// the server's own operation and, with `first_cycle` (the first check of a server process),
/// always. Returns the baseline in Unix milliseconds.
pub async fn begin_check(
    store: &StateStore,
    check_every: Duration,
    first_cycle: bool,
    now: OffsetDateTime,
) -> Result<Option<i64>, StateError> {
    Ok(store
        .alerting_heartbeat(now, check_every, first_cycle)
        .await?
        .map(to_ms))
}

/// The second step of a check: evaluates each host in its own transaction, refreshing the
/// heartbeat every 100 hosts and at the end so that a long cycle is not mistaken for a gap. A
/// host that fails is logged and skipped; only a failing host listing is an error. Safe to run
/// on several instances at once: `evaluate_host` locks the host and an open problem is unique.
pub async fn check_hosts(
    store: &StateStore,
    queue_mail: bool,
    baseline_ms: Option<i64>,
    now: OffsetDateTime,
) -> Result<usize, StateError> {
    let now_ms = to_ms(now);
    let hosts = store.hosts_with_nodata_triggers().await?;
    let mut checked = 0;
    for (n, host_id) in hosts.into_iter().enumerate() {
        if n > 0 && n % HEARTBEAT_EVERY_HOSTS == 0 {
            refresh_heartbeat(store).await;
        }
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
    refresh_heartbeat(store).await;
    Ok(checked)
}

/// Refreshes the heartbeat with the current time; a failure is logged, not fatal.
async fn refresh_heartbeat(store: &StateStore) {
    if let Err(err) = store.touch_heartbeat(OffsetDateTime::now_utc()).await {
        tracing::error!(%err, "no-data check: cannot refresh the heartbeat");
    }
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
    // The first check of this process moves the baseline whatever the stored heartbeat
    // says: agents have not reconnected yet, so silence counts from this start. Once that
    // step succeeded it is done, even if the rest of the cycle fails or panics: forcing the
    // baseline again on every cycle would keep any problem from ever opening.
    let mut first_cycle = true;

    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = ticker.tick() => {}
        }
        let now = OffsetDateTime::now_utc();

        let begun = {
            let store = Arc::clone(&store);
            let first = first_cycle;
            run_task(&shutdown, async move {
                begin_check(&store, check_every, first, now).await
            })
            .await
        };
        let baseline_ms = match begun {
            None => break,
            Some(Ok(Ok(baseline_ms))) => {
                first_cycle = false;
                baseline_ms
            }
            Some(Ok(Err(err))) => {
                tracing::error!(%err, "no-data check failed: cannot record the heartbeat");
                continue;
            }
            Some(Err(err)) => {
                tracing::error!(%err, "no-data check panicked");
                continue;
            }
        };

        let checked = {
            let store = Arc::clone(&store);
            run_task(&shutdown, async move {
                check_hosts(&store, queue_mail, baseline_ms, now).await
            })
            .await
        };
        match checked {
            None => break,
            Some(Ok(Ok(_))) => {}
            Some(Ok(Err(err))) => tracing::error!(%err, "no-data check failed"),
            Some(Err(err)) => tracing::error!(%err, "no-data check panicked"),
        }
    }
}

/// Runs `step` in a separate task, so that a panic in it is caught and returned; `None` when
/// `shutdown` was cancelled first. The aborted task is awaited, so that its transaction is
/// dropped (rolled back) before this returns.
async fn run_task<T, F>(shutdown: &CancellationToken, step: F) -> Option<Result<T, JoinError>>
where
    T: Send + 'static,
    F: std::future::Future<Output = T> + Send + 'static,
{
    let mut task = tokio::spawn(step);
    tokio::select! {
        () = shutdown.cancelled() => {
            task.abort();
            let _ = task.await;
            None
        }
        joined = &mut task => Some(joined),
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
    fn agent_outage_is_one_problem_whatever_the_windows() {
        let host10 = trig("Host silent", "no data for 10m", Severity::Critical);
        let disk2 = trig(
            "Disk silent",
            "disk.used_percent no data for 2m",
            Severity::Warning,
        );
        let host3 = host_trigger();
        let disk10 = trig(
            "Disk silent",
            "disk.used_percent no data for 10m",
            Severity::Warning,
        );
        // Everything stops at T, the disk value having arrived a little before the host's last
        // value or at the same moment. Checked every minute from T to T + 20 min, the item
        // trigger never fires, whichever window is the larger.
        for (triggers, host_window) in [([&host10, &disk2], 10), ([&host3, &disk10], 3)] {
            let t: Vec<ActiveTrigger> = triggers.into_iter().cloned().collect();
            for disk_before_host_ms in [0, 30_000, 90_000] {
                let mut states: Vec<TriggerState> = Vec::new();
                for minutes in 0..=20 {
                    let since_stop = minutes * MIN;
                    let s = seen(
                        since_stop,
                        vec![disk("/", since_stop + disk_before_host_ms)],
                    );
                    let e = run(&t, &states, &s, None);
                    assert!(
                        e.open.iter().all(|o| o.trigger == "Host silent"),
                        "item problem at T+{minutes}m: {e:?}"
                    );
                    if minutes == host_window {
                        assert_eq!(e.open.len(), 1, "host problem at T+{minutes}m");
                    } else {
                        assert!(e.open.is_empty(), "nothing new at T+{minutes}m: {e:?}");
                    }
                    states.extend(e.states);
                }
            }
        }

        // The case of the specification: item window 2 min, host window 10 min.
        let t = [host10.clone(), disk2.clone()];
        let e = run(&t, &[], &seen(3 * MIN, vec![disk("/", 3 * MIN)]), None);
        assert_eq!(e, Evaluation::default());
        let e = run(&t, &[], &seen(10 * MIN, vec![disk("/", 10 * MIN)]), None);
        let opened: Vec<&str> = e.open.iter().map(|o| o.trigger.as_str()).collect();
        assert_eq!(opened, ["Host silent"]);
    }

    #[test]
    fn item_that_stopped_before_the_host_still_alarms() {
        let t = [host_trigger(), disk_trigger()];
        // The disk has been silent for 6 minutes (window 5), the host reported 30 s ago.
        let s = seen(MIN / 2, vec![disk("/", 6 * MIN)]);
        let e = run(&t, &[], &s, None);
        assert_eq!(e.open.len(), 1);
        assert_eq!(e.open[0].trigger, "Disk silent");
        assert_eq!(e.open[0].instance, "mount=/");
        // Silent for 6 minutes, but the host's last value is only 4 minutes after the disk's:
        // when the host last reported the disk was not silent for its window yet.
        let s = seen(2 * MIN, vec![disk("/", 6 * MIN)]);
        assert!(run(&t, &[], &s, None).open.is_empty());
    }

    #[test]
    fn item_silence_counts_from_the_baseline() {
        let t = [host_trigger(), disk_trigger()];
        // The disk has been silent for an hour, the host reports, but the server only started
        // 4 minutes ago: not yet. Seven minutes after the start: now it alarms.
        let s = seen(0, vec![disk("/", 60 * MIN)]);
        assert!(run(&t, &[], &s, Some(NOW - 4 * MIN)).open.is_empty());
        assert_eq!(run(&t, &[], &s, Some(NOW - 7 * MIN)).open.len(), 1);
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
    fn server_gap_keeps_open_problems_host_level() {
        let t = [host_trigger()];
        let st = [firing("Host silent", "")];
        // Silent for 20 minutes; the server only just came back (baseline = now).
        let e = run(&t, &st, &seen(20 * MIN, vec![]), Some(NOW));
        assert_eq!(e, Evaluation::default());
    }

    #[test]
    fn server_gap_keeps_open_problems_item_level() {
        let t = [host_trigger(), disk_trigger()];
        let st = [firing("Disk silent", "mount=/")];
        // The host reports, the disk has been silent for 20 minutes, baseline = now.
        let s = seen(0, vec![disk("/", 20 * MIN)]);
        let e = run(&t, &st, &s, Some(NOW));
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
        store
            .apply_config("c1", &compiled, t0(), 50, false)
            .await
            .unwrap();
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

    async fn baseline_of(pool: &PgPool) -> Option<OffsetDateTime> {
        sqlx::query_scalar("SELECT baseline_at FROM alerting_state")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn server_gap_keeps_open_problems(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        store.touch_seen(host, after(-1200), &[]).await.unwrap();
        set_heartbeat(&pool, after(-60)).await;
        check_once(&store, false, EVERY, false, t0()).await.unwrap();
        assert_eq!(open_problems(&store).await.len(), 1);

        // The server was away: the heartbeat is far in the past, the baseline moves.
        set_heartbeat(&pool, after(-3600)).await;
        check_once(&store, false, EVERY, false, after(30))
            .await
            .unwrap();
        assert_eq!(baseline_of(&pool).await, Some(after(30)));
        let all = store.problems(ProblemFilter::All, 10).await.unwrap();
        assert_eq!(all.len(), 1, "no new problem");
        assert_eq!(all[0].resolved_at, None, "still open");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn slow_cycle_is_not_a_gap(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        store.touch_seen(host, after(-600), &[]).await.unwrap();
        // The previous cycle started long ago but refreshed the heartbeat a few seconds ago.
        set_heartbeat(&pool, after(-300)).await;
        store.touch_heartbeat(after(-5)).await.unwrap();
        check_once(&store, false, EVERY, false, t0()).await.unwrap();
        assert_eq!(baseline_of(&pool).await, None);
        assert_eq!(open_problems(&store).await.len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn first_cycle_after_a_short_outage_does_not_alarm(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        store.touch_seen(host, after(-300), &[]).await.unwrap();
        // A fresh heartbeat (no gap by the 3 x rule), but this is the first cycle.
        set_heartbeat(&pool, after(-10)).await;
        check_once(&store, false, EVERY, true, t0()).await.unwrap();
        assert_eq!(baseline_of(&pool).await, Some(t0()));
        assert!(open_problems(&store).await.is_empty());
        // Three minutes after the start, still nothing arrived: now it alarms.
        check_once(&store, false, EVERY, false, after(180))
            .await
            .unwrap();
        assert_eq!(open_problems(&store).await.len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn check_once_opens_and_resolves_end_to_end(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        // web-01 last reported 5 minutes ago; db-01 never did. The check ran a minute ago.
        store.touch_seen(host, after(-300), &[]).await.unwrap();
        set_heartbeat(&pool, after(-60)).await;

        let checked = check_once(&store, false, EVERY, false, t0()).await.unwrap();
        assert_eq!(checked, 2);
        assert_eq!(
            open_problems(&store).await,
            [("web-01".to_owned(), "Agent silent".to_owned())]
        );
        let p = &store.problems(ProblemFilter::Open, 10).await.unwrap()[0];
        assert_eq!(p.summary, "no data for 3m");
        assert_eq!(p.started_at, t0());

        // The same silence again: still one problem.
        check_once(&store, false, EVERY, false, after(30))
            .await
            .unwrap();
        assert_eq!(open_problems(&store).await.len(), 1);

        store.touch_seen(host, after(40), &[]).await.unwrap();
        check_once(&store, false, EVERY, false, after(60))
            .await
            .unwrap();
        assert!(open_problems(&store).await.is_empty());
        let all = store.problems(ProblemFilter::All, 10).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].resolved_at, Some(after(60)));
    }

    /// The example configuration, with or without "Root silent"
    /// (`disk.used_percent{mount=/} no data for 5m`) on web-01.
    fn example_with_root_trigger(on: bool) -> crate::gitops::Compiled {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        let mut compiled = compile(&read_tree(&dir, "c1").unwrap()).unwrap();
        if on {
            let web = compiled
                .hosts
                .iter_mut()
                .find(|h| h.name == "web-01")
                .unwrap();
            web.effective
                .triggers
                .push(crate::gitops::EffectiveTrigger {
                    name: "Root silent".into(),
                    expression: "disk.used_percent{mount=/} no data for 5m".into(),
                    severity: Severity::Warning,
                    origin: "host".into(),
                });
        }
        compiled
    }

    fn root_series() -> Vec<(String, Params)> {
        vec![("disk.used_percent".into(), mount("/"))]
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn restored_item_nodata_trigger_does_not_alarm(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        store
            .apply_config("c1", &example_with_root_trigger(true), t0(), 50, false)
            .await
            .unwrap();
        let host = store.host_by_name("web-01").await.unwrap().unwrap().id;
        store.touch_seen(host, t0(), &root_series()).await.unwrap();
        // The trigger is removed, and restored an hour later (a revert of the commit).
        store
            .apply_config(
                "c2",
                &example_with_root_trigger(false),
                after(60),
                50,
                false,
            )
            .await
            .unwrap();
        store
            .apply_config(
                "c3",
                &example_with_root_trigger(true),
                after(3600),
                50,
                false,
            )
            .await
            .unwrap();
        // The host keeps reporting; nothing is known about the disk since the restore.
        store.touch_seen(host, after(3590), &[]).await.unwrap();
        set_heartbeat(&pool, after(3540)).await;
        check_once(&store, false, EVERY, false, after(3600))
            .await
            .unwrap();
        assert!(open_problems(&store).await.is_empty());

        // A disk value arrives, then the disk stops while the host keeps reporting.
        store
            .touch_seen(host, after(3610), &root_series())
            .await
            .unwrap();
        store.touch_seen(host, after(4010), &[]).await.unwrap();
        set_heartbeat(&pool, after(3950)).await;
        check_once(&store, false, EVERY, false, after(4010))
            .await
            .unwrap();
        assert_eq!(
            open_problems(&store).await,
            [("web-01".to_owned(), "Root silent".to_owned())]
        );
    }

    async fn heartbeat_of(pool: &PgPool) -> Option<OffsetDateTime> {
        sqlx::query_scalar("SELECT heartbeat_at FROM alerting_state")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn failure_after_the_heartbeat_does_not_force_the_baseline_again(pool: PgPool) {
        let (store, _) = setup(pool.clone()).await;
        set_heartbeat(&pool, OffsetDateTime::now_utc()).await;
        // Every cycle records its heartbeat and then fails to list the hosts.
        sqlx::query("ALTER TABLE hosts RENAME COLUMN config TO config_gone")
            .execute(&pool)
            .await
            .unwrap();
        let shutdown = CancellationToken::new();
        // Two seconds between checks: a gap (three intervals) needs a stall of six seconds.
        let handle = tokio::spawn(run_nodata_loop(
            store.clone(),
            false,
            Duration::from_secs(2),
            shutdown.clone(),
        ));
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        // The first cycle forces the baseline to its start ...
        let first = loop {
            if let Some(at) = baseline_of(&pool).await {
                break at;
            }
            assert!(std::time::Instant::now() < deadline, "the first check ran");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        // ... and a later cycle, whose heartbeat is newer, keeps it.
        while heartbeat_of(&pool).await <= Some(first) {
            assert!(std::time::Instant::now() < deadline, "a second check ran");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(baseline_of(&pool).await, Some(first));
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(30), handle)
            .await
            .expect("loop stops after shutdown")
            .expect("the loop itself did not panic");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn server_gap_does_not_alarm(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        store.touch_seen(host, after(-600), &[]).await.unwrap();
        set_heartbeat(&pool, after(-600)).await;

        // The server was down for 10 minutes: the host is not silent yet.
        check_once(&store, false, EVERY, false, t0()).await.unwrap();
        assert!(open_problems(&store).await.is_empty());

        // Four minutes later, still without a value: now it is.
        check_once(&store, false, EVERY, false, after(240))
            .await
            .unwrap();
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
        let checked = check_once(&store, false, EVERY, false, t0()).await.unwrap();
        assert_eq!(checked, 1);
        assert_eq!(open_problems(&store).await.len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn loop_stops_on_shutdown(pool: PgPool) {
        let (store, host) = setup(pool.clone()).await;
        // Silent for an hour and a fresh heartbeat. The first cycle of a process still moves
        // the baseline to now, so no problem opens: silence counts from the server start.
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
        let mut ran = false;
        for _ in 0..600 {
            if baseline_of(&pool).await.is_some() {
                ran = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(30), handle)
            .await
            .expect("loop stops after shutdown")
            .expect("the loop itself did not panic");
        assert!(ran, "the first check ran and set the baseline");
        assert!(open_problems(&store).await.is_empty());
    }
}
