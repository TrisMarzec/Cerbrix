//! Pure evaluation of comparison triggers.

use std::collections::{BTreeMap, HashMap};

use cerbrix_common::{Comparison, MetricValue, TriggerExpr};

use super::{ActiveTrigger, Evaluation, OpenProblem, ResolveProblem, TriggerState, instance_key};

/// Decides from the stored trigger state and newly arrived values which problems
/// open and which resolve.
///
/// Only `TriggerExpr::Compare` triggers are evaluated. A value older than
/// `now_ms - max_age_ms` that matches at least one trigger is counted in
/// `too_old` (once) and otherwise ignored. The result is deterministic: `states`,
/// `open` and `resolve` are ordered by trigger name, then instance, then time.
/// The function never panics; all time arithmetic saturates.
pub fn evaluate(
    triggers: &[ActiveTrigger],
    states: &[TriggerState],
    values: &[MetricValue],
    now_ms: i64,
    max_age_ms: i64,
) -> Evaluation {
    // Index the comparison triggers by item key.
    let mut by_key: HashMap<&str, Vec<&ActiveTrigger>> = HashMap::new();
    for t in triggers {
        if let TriggerExpr::Compare { selector, .. } = &t.expr {
            by_key.entry(selector.key.as_str()).or_default().push(t);
        }
    }
    let mut stored: HashMap<(&str, &str), &TriggerState> = HashMap::new();
    for s in states {
        stored
            .entry((s.trigger.as_str(), s.instance.as_str()))
            .or_insert(s);
    }

    let oldest_allowed = now_ms.saturating_sub(max_age_ms);
    let mut out = Evaluation::default();
    // (trigger name, instance) -> (trigger, values in arrival order)
    let mut groups: BTreeMap<(&str, String), (&ActiveTrigger, Vec<&MetricValue>)> = BTreeMap::new();

    for v in values {
        let Some(candidates) = by_key.get(v.key.as_str()) else {
            continue;
        };
        let mut matched = false;
        for t in candidates {
            let TriggerExpr::Compare { selector, .. } = &t.expr else {
                continue;
            };
            if !selector
                .params
                .iter()
                .all(|(k, want)| v.params.get(k) == Some(want))
            {
                continue;
            }
            matched = true;
            if v.ts_ms < oldest_allowed {
                break;
            }
            groups
                .entry((t.name.as_str(), instance_key(&v.params)))
                .or_insert_with(|| (t, Vec::new()))
                .1
                .push(v);
        }
        if matched && v.ts_ms < oldest_allowed {
            out.too_old += 1;
        }
    }

    for ((name, instance), (trigger, mut vals)) in groups {
        let TriggerExpr::Compare {
            op,
            threshold,
            hold,
            ..
        } = &trigger.expr
        else {
            continue;
        };
        let hold_ms = hold
            .filter(|d| !d.is_zero())
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));

        let mut st = match stored.get(&(name, instance.as_str())) {
            Some(s) => (*s).clone(),
            None => TriggerState {
                trigger: name.to_string(),
                instance: instance.clone(),
                firing: false,
                condition_since_ms: None,
                last_ts_ms: i64::MIN,
                last_value: None,
            },
        };
        // Stable sort: of equal timestamps the first one in the batch wins.
        vals.sort_by_key(|v| v.ts_ms);

        let mut changed = false;
        for v in vals {
            if v.ts_ms <= st.last_ts_ms {
                continue;
            }
            changed = true;
            st.last_ts_ms = v.ts_ms;
            st.last_value = Some(v.value);
            if compare(*op, v.value, *threshold) {
                let since = *st.condition_since_ms.get_or_insert(v.ts_ms);
                let held = hold_ms.is_none_or(|h| v.ts_ms.saturating_sub(since) >= h);
                if held && !st.firing {
                    st.firing = true;
                    out.open.push(OpenProblem {
                        trigger: name.to_string(),
                        instance: instance.clone(),
                        severity: trigger.severity,
                        at_ms: v.ts_ms,
                        summary: summary(&trigger.expression, v.value, &instance),
                    });
                }
            } else {
                st.condition_since_ms = None;
                if st.firing {
                    st.firing = false;
                    out.resolve.push(ResolveProblem {
                        trigger: name.to_string(),
                        instance: instance.clone(),
                        at_ms: v.ts_ms,
                    });
                }
            }
        }
        if changed {
            out.states.push(st);
        }
    }
    out
}

fn compare(op: Comparison, value: f64, threshold: f64) -> bool {
    match op {
        Comparison::Gt => value > threshold,
        Comparison::Lt => value < threshold,
        Comparison::Ge => value >= threshold,
        Comparison::Le => value <= threshold,
        Comparison::Eq => value == threshold,
        Comparison::Ne => value != threshold,
    }
}

fn summary(expression: &str, value: f64, instance: &str) -> String {
    let mut s = format!("{expression}: value {}", format_value(value));
    if !instance.is_empty() {
        s.push_str(" on ");
        s.push_str(instance);
    }
    s
}

/// At most 3 decimals, no trailing zeros.
fn format_value(value: f64) -> String {
    let mut s = format!("{value:.3}");
    if s.contains('.') {
        s.truncate(s.trim_end_matches('0').trim_end_matches('.').len());
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerting::{
        ActiveTrigger, Evaluation, Params, TriggerState, active_triggers, instance_key,
    };
    use crate::gitops::{EffectiveHost, EffectiveTrigger};
    use cerbrix_common::config::Severity;
    use cerbrix_common::{MetricValue, parse_trigger};

    const NOW: i64 = 1_000_000_000;
    const MAX_AGE: i64 = 300_000;
    const MIN: i64 = 60_000;

    fn trig(name: &str, expr: &str, severity: Severity) -> ActiveTrigger {
        ActiveTrigger {
            name: name.into(),
            severity,
            expression: expr.into(),
            expr: parse_trigger(expr).unwrap(),
        }
    }

    fn params(pairs: &[(&str, &str)]) -> Params {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn val(key: &str, p: &[(&str, &str)], ts_ms: i64, value: f64) -> MetricValue {
        MetricValue {
            key: key.into(),
            params: params(p),
            ts_ms,
            value,
        }
    }

    fn state(
        trigger: &str,
        instance: &str,
        firing: bool,
        since: Option<i64>,
        last: i64,
    ) -> TriggerState {
        TriggerState {
            trigger: trigger.into(),
            instance: instance.into(),
            firing,
            condition_since_ms: since,
            last_ts_ms: last,
            last_value: Some(0.0),
        }
    }

    fn ev(t: &[ActiveTrigger], s: &[TriggerState], v: &[MetricValue]) -> Evaluation {
        evaluate(t, s, v, NOW, MAX_AGE)
    }

    #[test]
    fn instance_key_is_canonical() {
        assert_eq!(instance_key(&Params::new()), "");
        assert_eq!(instance_key(&params(&[("b", "2"), ("a", "1")])), "a=1,b=2");
    }

    #[test]
    fn fires_without_hold() {
        let t = [trig("cpu", "cpu.usage > 90", Severity::High)];
        let at = NOW - 1000;
        let r = ev(&t, &[], &[val("cpu.usage", &[], at, 95.0)]);
        assert_eq!(r.open.len(), 1);
        assert_eq!(r.open[0].at_ms, at);
        assert_eq!(r.open[0].severity, Severity::High);
        assert_eq!(r.open[0].summary, "cpu.usage > 90: value 95");
        assert!(r.resolve.is_empty());
        assert_eq!(r.states.len(), 1);
        assert!(r.states[0].firing);
        assert_eq!(r.states[0].last_ts_ms, at);
        assert_eq!(r.states[0].last_value, Some(95.0));
    }

    #[test]
    fn does_not_fire_below_threshold() {
        let t = [trig("cpu", "cpu.usage > 90", Severity::High)];
        let r = ev(&t, &[], &[val("cpu.usage", &[], NOW - 1000, 90.0)]);
        assert!(r.open.is_empty());
        assert_eq!(r.states.len(), 1);
        assert!(!r.states[0].firing);
        assert_eq!(r.states[0].condition_since_ms, None);
    }

    #[test]
    fn every_operator() {
        // (expression, results for 89, 90, 91)
        let cases = [
            ("m > 90", [false, false, true]),
            ("m < 90", [true, false, false]),
            ("m >= 90", [false, true, true]),
            ("m <= 90", [true, true, false]),
            ("m == 90", [false, true, false]),
            ("m != 90", [true, false, true]),
        ];
        for (expr, expected) in cases {
            for (v, want) in [89.0, 90.0, 91.0].into_iter().zip(expected) {
                let t = [trig("t", expr, Severity::Info)];
                let r = ev(&t, &[], &[val("m", &[], NOW - 1000, v)]);
                assert_eq!(r.open.len() == 1, want, "{expr} with {v}");
            }
        }
    }

    #[test]
    fn hold_needs_the_full_duration() {
        let t = [trig("d", "m > 90 for 10m", Severity::Warning)];
        let s = NOW - 20 * MIN;
        let all = [
            val("m", &[], s, 95.0),
            val("m", &[], s + 5 * MIN, 95.0),
            val("m", &[], s + 10 * MIN, 95.0),
        ];
        let r = evaluate(&t, &[], &all, NOW, 60 * MIN);
        assert_eq!(r.open.len(), 1);
        assert_eq!(r.open[0].at_ms, s + 10 * MIN);

        let r = evaluate(&t, &[], &all[..2], NOW, 60 * MIN);
        assert!(r.open.is_empty());
        assert_eq!(r.states[0].condition_since_ms, Some(s));
        assert!(!r.states[0].firing);
    }

    #[test]
    fn hold_continues_from_stored_state() {
        let t = [trig("d", "m > 90 for 10m", Severity::Warning)];
        let s = NOW - 20 * MIN;
        let st = [state("d", "", false, Some(s), s + 5 * MIN)];
        let r = evaluate(&t, &st, &[val("m", &[], s + 10 * MIN, 95.0)], NOW, 60 * MIN);
        assert_eq!(r.open.len(), 1);
        assert_eq!(r.open[0].at_ms, s + 10 * MIN);
    }

    #[test]
    fn dip_resets_the_hold() {
        let t = [trig("d", "m > 90 for 10m", Severity::Warning)];
        let s = NOW - 20 * MIN;
        let vals = [
            val("m", &[], s, 95.0),
            val("m", &[], s + 4 * MIN, 80.0),
            val("m", &[], s + 8 * MIN, 95.0),
        ];
        let r = evaluate(&t, &[], &vals, NOW, 60 * MIN);
        assert!(r.open.is_empty());
        assert_eq!(r.states[0].condition_since_ms, Some(s + 8 * MIN));
    }

    #[test]
    fn resolves_when_condition_ends() {
        let t = [trig("cpu", "cpu.usage > 90", Severity::High)];
        let st = [state("cpu", "", true, Some(NOW - 5000), NOW - 5000)];
        let at = NOW - 1000;
        let r = ev(&t, &st, &[val("cpu.usage", &[], at, 50.0)]);
        assert!(r.open.is_empty());
        assert_eq!(r.resolve.len(), 1);
        assert_eq!(r.resolve[0].at_ms, at);
        assert!(!r.states[0].firing);
        assert_eq!(r.states[0].condition_since_ms, None);
    }

    #[test]
    fn no_second_open_while_firing() {
        let t = [trig("cpu", "cpu.usage > 90", Severity::High)];
        let st = [state("cpu", "", true, Some(NOW - 5000), NOW - 5000)];
        let r = ev(&t, &st, &[val("cpu.usage", &[], NOW - 1000, 99.0)]);
        assert!(r.open.is_empty());
        assert!(r.resolve.is_empty());
        assert_eq!(r.states.len(), 1);
        assert!(r.states[0].firing);
        assert_eq!(r.states[0].last_value, Some(99.0));
        assert_eq!(r.states[0].last_ts_ms, NOW - 1000);
        assert_eq!(r.states[0].condition_since_ms, Some(NOW - 5000));
    }

    #[test]
    fn per_instance() {
        let t = [trig("disk", "disk.used_percent > 90", Severity::High)];
        let r = ev(
            &t,
            &[],
            &[
                val("disk.used_percent", &[("mount", "/data")], NOW - 1000, 40.0),
                val("disk.used_percent", &[("mount", "/")], NOW - 1000, 96.0),
            ],
        );
        assert_eq!(r.open.len(), 1);
        assert_eq!(r.open[0].instance, "mount=/");
        assert_eq!(r.states.len(), 2);
        assert_eq!(r.states[0].instance, "mount=/");
        assert_eq!(r.states[1].instance, "mount=/data");
    }

    #[test]
    fn selector_params_narrow() {
        let t = [trig(
            "disk",
            "disk.used_percent{mount=/} > 90",
            Severity::High,
        )];
        let r = ev(
            &t,
            &[],
            &[val(
                "disk.used_percent",
                &[("mount", "/data")],
                NOW - 1000,
                99.0,
            )],
        );
        assert!(r.open.is_empty());
        assert!(r.states.is_empty());
    }

    #[test]
    fn too_old_values_are_counted_not_evaluated() {
        let t = [trig("cpu", "cpu.usage > 90", Severity::High)];
        let r = ev(&t, &[], &[val("cpu.usage", &[], NOW - 301_000, 99.0)]);
        assert_eq!(r.too_old, 1);
        assert!(r.states.is_empty());
        assert!(r.open.is_empty());
    }

    #[test]
    fn duplicate_and_out_of_order_values_are_ignored() {
        let t = [trig("cpu", "cpu.usage > 90", Severity::High)];
        let at = NOW - 10_000;
        let st = [state("cpu", "", false, None, at)];
        let r = ev(
            &t,
            &st,
            &[
                val("cpu.usage", &[], at, 99.0),
                val("cpu.usage", &[], at - 1, 99.0),
            ],
        );
        assert_eq!(r, Evaluation::default());

        let r = ev(
            &t,
            &st,
            &[
                val("cpu.usage", &[], at + 2, 50.0),
                val("cpu.usage", &[], at + 1, 95.0),
            ],
        );
        assert_eq!(r.open.len(), 1);
        assert_eq!(r.open[0].at_ms, at + 1);
        assert_eq!(r.resolve.len(), 1);
        assert_eq!(r.resolve[0].at_ms, at + 2);
        assert!(!r.states[0].firing);
    }

    #[test]
    fn resending_a_batch_changes_nothing() {
        let t = [
            trig("cpu", "cpu.usage > 90", Severity::High),
            trig("h", "cpu.usage > 10 for 1m", Severity::Info),
        ];
        let batch = [
            val("cpu.usage", &[], NOW - 3000, 95.0),
            val("cpu.usage", &[], NOW - 2000, 50.0),
            val("cpu.usage", &[], NOW - 1000, 97.0),
        ];
        let first = ev(&t, &[], &batch);
        assert!(!first.states.is_empty());
        let second = ev(&t, &first.states, &batch);
        assert!(second.open.is_empty());
        assert!(second.resolve.is_empty());
        assert!(second.states.is_empty());
    }

    #[test]
    fn nodata_triggers_are_ignored() {
        let t = [trig("nd", "cpu.usage no data for 5m", Severity::High)];
        let r = ev(&t, &[], &[val("cpu.usage", &[], NOW - 1000, 99.0)]);
        assert_eq!(r, Evaluation::default());
    }

    #[test]
    fn unparsable_trigger_is_skipped() {
        let host = EffectiveHost {
            templates: vec![],
            items: vec![],
            triggers: vec![
                EffectiveTrigger {
                    name: "bad".into(),
                    expression: "this is not a trigger".into(),
                    severity: Severity::Info,
                    origin: "host".into(),
                },
                EffectiveTrigger {
                    name: "good".into(),
                    expression: "cpu.usage > 90".into(),
                    severity: Severity::High,
                    origin: "host".into(),
                },
            ],
        };
        let a = active_triggers(&host);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].name, "good");
    }

    #[test]
    fn summary_formats_value_and_instance() {
        let t = [trig("d", "disk.used_percent > 90 for 10m", Severity::High)];
        let s = NOW - 20 * MIN;
        let st = [state("d", "mount=/", false, Some(s), s)];
        let r = evaluate(
            &t,
            &st,
            &[val(
                "disk.used_percent",
                &[("mount", "/")],
                s + 10 * MIN,
                96.5,
            )],
            NOW,
            60 * MIN,
        );
        assert_eq!(
            r.open[0].summary,
            "disk.used_percent > 90 for 10m: value 96.5 on mount=/"
        );

        let t = [trig("a", "m > 0", Severity::Info)];
        let r = ev(&t, &[], &[val("m", &[], NOW - 1000, 95.0)]);
        assert_eq!(r.open[0].summary, "m > 0: value 95");
        let r = ev(&t, &[], &[val("m", &[], NOW - 1000, 0.123456)]);
        assert_eq!(r.open[0].summary, "m > 0: value 0.123");
        let r = ev(&t, &[], &[val("m", &[], NOW - 1000, -0.0001)]);
        assert_eq!(r.open.len(), 0);
        let t = [trig("a", "m < 0", Severity::Info)];
        let r = ev(&t, &[], &[val("m", &[], NOW - 1000, -0.0001)]);
        assert_eq!(r.open[0].summary, "m < 0: value 0");
    }

    #[test]
    fn extreme_inputs_do_not_panic() {
        let t = [trig("a", "m > 0 for 10m", Severity::Info)];
        let vals = [
            val("m", &[], i64::MIN, 1.0),
            val("m", &[], i64::MAX, 1.0),
            val("m", &[], 0, 1.0),
        ];
        let _ = evaluate(&t, &[], &vals, i64::MIN, i64::MAX);
        let _ = evaluate(&t, &[], &vals, i64::MAX, i64::MIN);
        let _ = evaluate(&t, &[], &vals, 0, i64::MAX);
        let st = [state("a", "", false, Some(i64::MIN), i64::MIN)];
        let _ = evaluate(&t, &st, &vals, 0, i64::MAX);
    }

    #[test]
    fn output_order_is_by_trigger_then_instance_then_time() {
        let t = [
            trig("b", "m > 0", Severity::Info),
            trig("a", "m > 0", Severity::Info),
        ];
        let r = ev(
            &t,
            &[],
            &[
                val("m", &[("i", "2")], NOW - 1000, 1.0),
                val("m", &[("i", "1")], NOW - 2000, 1.0),
            ],
        );
        let order: Vec<_> = r
            .open
            .iter()
            .map(|o| (o.trigger.as_str(), o.instance.as_str()))
            .collect();
        assert_eq!(
            order,
            [("a", "i=1"), ("a", "i=2"), ("b", "i=1"), ("b", "i=2")]
        );
    }
}
