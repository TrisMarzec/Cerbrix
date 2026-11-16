//! What the host detail and the dashboards share when they show a metric series: the time
//! limit of a call to the metrics store, the thresholds drawn into a graph, the query of a
//! range and the step chosen for it.

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use cerbrix_common::config::Severity;
use cerbrix_common::{Comparison, TriggerExpr, builtin_item};
use serde::Serialize;
use time::OffsetDateTime;

use super::rfc3339;
use crate::alerting::active_triggers;
use crate::gitops::EffectiveHost;
use crate::metrics::{MetricsError, RangeQuery};

/// The current value of an item is at most this old.
pub const CURRENT_VALUE_MAX_AGE_SECS: u32 = 3600;

/// The step of a graph is never finer than this.
pub const MIN_STEP_SECS: u32 = 15;

/// A graph asks for about this many points at most.
const TARGET_POINTS: u64 = 300;

/// The step for a graph over `range_secs`: the range divided into about 300 points, but at
/// least [`MIN_STEP_SECS`] (the agents do not report faster than that in practice).
pub fn step_for(range_secs: u64) -> u32 {
    let step = range_secs.div_ceil(TARGET_POINTS);
    u32::try_from(step).map_or(u32::MAX, |s| s.max(MIN_STEP_SECS))
}

/// Runs a call to the metrics store for at most `limit`; running out of time is the same
/// as the store being unavailable.
pub async fn bounded<T>(
    limit: Duration,
    call: impl Future<Output = Result<T, MetricsError>>,
) -> Result<T, MetricsError> {
    tokio::time::timeout(limit, call)
        .await
        .unwrap_or_else(|_| Err(MetricsError::Unavailable("timed out".into())))
}

/// Logs a failed call to the metrics store: an outage is expected now and then, a refused
/// query is a bug.
pub fn log_metrics_failure(what: &str, e: &MetricsError) {
    match e {
        MetricsError::Unavailable(_) => tracing::warn!(error = %e, "{what}: metrics unavailable"),
        MetricsError::Rejected(_) => tracing::error!(error = %e, "{what}: query refused"),
    }
}

/// The range query of one series from `from` to `to`.
pub fn range_query(
    host: String,
    key: String,
    params: BTreeMap<String, String>,
    from: OffsetDateTime,
    to: OffsetDateTime,
    step_secs: u32,
) -> RangeQuery {
    let ms = |t: OffsetDateTime| (t.unix_timestamp_nanos() / 1_000_000) as i64;
    RangeQuery {
        host,
        key,
        params,
        from_ms: ms(from),
        to_ms: ms(to),
        step_secs,
    }
}

/// A sample time in Unix milliseconds as RFC 3339; `None` for a time out of range.
pub fn ms_to_rfc3339(ms: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .map(rfc3339)
}

/// Whether `params` name exactly one series of the item: the one parameter an item with a
/// parameter has (without it the selector would match every instance and the store refuses
/// the query), none for an item without one. An item that is not built in is not known well
/// enough to check.
pub fn names_one_series(item: &str, params: &BTreeMap<String, String>) -> bool {
    match builtin_item(item) {
        None => true,
        Some(spec) => match spec.param {
            Some(want) => params.len() == 1 && params.contains_key(want),
            None => params.is_empty(),
        },
    }
}

/// A trigger limit drawn into the graph of its item.
#[derive(Debug, Serialize)]
pub struct Threshold {
    pub trigger: String,
    pub severity: Severity,
    pub op: &'static str,
    pub value: f64,
}

fn op_text(op: Comparison) -> &'static str {
    match op {
        Comparison::Gt => ">",
        Comparison::Ge => ">=",
        Comparison::Lt => "<",
        Comparison::Le => "<=",
        Comparison::Eq => "==",
        Comparison::Ne => "!=",
    }
}

/// The comparison triggers of the host on `key` whose selector fits the requested
/// parameters: every parameter of the selector is requested with the same value. A selector
/// without parameters fits every request.
pub fn thresholds(
    config: &EffectiveHost,
    key: &str,
    params: &BTreeMap<String, String>,
) -> Vec<Threshold> {
    active_triggers(config)
        .into_iter()
        .filter_map(|t| match t.expr {
            TriggerExpr::Compare {
                selector,
                op,
                threshold,
                ..
            } if selector.key == key
                && selector
                    .params
                    .iter()
                    .all(|(name, want)| params.get(name) == Some(want)) =>
            {
                Some(Threshold {
                    trigger: t.name,
                    severity: t.severity,
                    op: op_text(op),
                    value: threshold,
                })
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitops::EffectiveTrigger;

    #[test]
    fn thresholds_match_item_and_parameters() {
        let trigger = |name: &str, expression: &str, severity| EffectiveTrigger {
            name: name.into(),
            expression: expression.into(),
            severity,
            origin: "host".into(),
        };
        let config = EffectiveHost {
            templates: vec![],
            items: vec![],
            triggers: vec![
                trigger("any mount", "disk.used_percent > 90", Severity::High),
                trigger(
                    "root",
                    "disk.used_percent{mount=/} >= 80 for 5m",
                    Severity::Warning,
                ),
                trigger("data", "disk.used_percent{mount=/data} < 5", Severity::Info),
                trigger("other item", "cpu.usage > 50", Severity::High),
                trigger("silent", "no data for 3m", Severity::Critical),
                trigger("broken", "disk.used_percent >> 1", Severity::High),
                trigger("eq", "disk.used_percent == 1.5", Severity::High),
                trigger("ne", "disk.used_percent != 2", Severity::High),
                trigger("le", "disk.used_percent <= 3", Severity::High),
                trigger("both", "disk.used_percent{mount=/,x=y} > 1", Severity::High),
            ],
        };
        let params = |p: &[(&str, &str)]| -> BTreeMap<String, String> {
            p.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let names = |p: &[(&str, &str)]| -> Vec<(String, &'static str)> {
            thresholds(&config, "disk.used_percent", &params(p))
                .into_iter()
                .map(|t| (t.trigger, t.op))
                .collect()
        };
        let any = |op: &'static str| ("any mount".to_owned(), op);
        let tail = [
            ("eq".to_owned(), "=="),
            ("ne".to_owned(), "!="),
            ("le".to_owned(), "<="),
        ];
        let mut root = vec![any(">"), ("root".to_owned(), ">=")];
        root.extend(tail.clone());
        assert_eq!(names(&[("mount", "/")]), root);
        let mut data = vec![any(">"), ("data".to_owned(), "<")];
        data.extend(tail.clone());
        assert_eq!(names(&[("mount", "/data")]), data);
        // A selector with parameters needs them in the request.
        let mut bare = vec![any(">")];
        bare.extend(tail.clone());
        assert_eq!(names(&[]), bare);
        let mut both = vec![any(">"), ("root".to_owned(), ">=")];
        both.extend(tail);
        both.push(("both".to_owned(), ">"));
        assert_eq!(names(&[("mount", "/"), ("x", "y")]), both);
        assert_eq!(
            thresholds(&config, "cpu.usage", &BTreeMap::new())[0].trigger,
            "other item"
        );
        assert!(thresholds(&config, "memory.used_percent", &BTreeMap::new()).is_empty());
    }

    #[tokio::test]
    async fn a_metrics_call_is_bounded() {
        let slow = bounded(Duration::from_millis(50), async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(1)
        })
        .await;
        assert!(
            matches!(slow, Err(MetricsError::Unavailable(_))),
            "{slow:?}"
        );
        let fast = bounded(Duration::from_secs(5), async { Ok(1) }).await;
        assert_eq!(fast.unwrap(), 1);
        let failed: Result<i32, _> = bounded(Duration::from_secs(5), async {
            Err(MetricsError::Rejected("x".into()))
        })
        .await;
        assert!(matches!(failed, Err(MetricsError::Rejected(_))));
    }

    #[test]
    fn step_gives_about_300_points_and_never_less_than_15_seconds() {
        for (secs, step) in [
            (1, 15),
            (60, 15),
            (3_600, 15),
            (4_500, 15),
            (4_501, 16),
            (21_600, 72),
            (86_400, 288),
        ] {
            assert_eq!(step_for(secs), step, "{secs} s");
        }
        for secs in [1_u64, 59, 3_600, 21_600, 50_000, 86_400] {
            let points = secs / u64::from(step_for(secs));
            assert!(points <= 300, "{secs} s gives {points} points");
        }
    }

    #[test]
    fn a_series_is_named_by_exactly_the_parameter_of_its_item() {
        let p = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert!(names_one_series("disk.used_percent", &p(&[("mount", "/")])));
        assert!(!names_one_series("disk.used_percent", &p(&[])));
        assert!(!names_one_series(
            "disk.used_percent",
            &p(&[("interface", "eth0")])
        ));
        assert!(!names_one_series(
            "disk.used_percent",
            &p(&[("mount", "/"), ("x", "y")])
        ));
        assert!(names_one_series(
            "net.rx_bytes_per_sec",
            &p(&[("interface", "eth0")])
        ));
        assert!(names_one_series("cpu.usage", &p(&[])));
        assert!(!names_one_series("cpu.usage", &p(&[("mount", "/")])));
        assert!(names_one_series("custom.thing", &p(&[("a", "b")])));
    }

    #[test]
    fn sample_times_are_rfc3339_in_utc() {
        assert_eq!(
            ms_to_rfc3339(1_800_000_000_000).as_deref(),
            Some("2027-01-15T08:00:00Z")
        );
        assert_eq!(ms_to_rfc3339(i64::MAX), None);
    }
}
