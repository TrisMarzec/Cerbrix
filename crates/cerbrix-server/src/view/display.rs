//! Wall displays: which dashboards a display may show and which one an alarm switches it
//! to (UI concept, section 6). Both are pure functions of the display file and the stored
//! state, so every instance gives the same answer and the server keeps nothing per display.

use std::cmp::Reverse;
use std::collections::BTreeSet;

use cerbrix_common::config::Severity;
use cerbrix_common::{DisplayFile, Show};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use super::dashboard::{selected_hosts, urgency};
use crate::state::{BoardInputs, Problem};

/// What an alarm makes a display do.
#[derive(Serialize, PartialEq, Debug)]
pub struct DisplayTarget {
    /// The dashboard the display switches to; `None` when it shows its `show` setting.
    pub target: Option<String>,
    /// The alarms of the target, most severe first.
    pub causes: Vec<Uuid>,
    /// The other dashboards with an alarm, in the order the display would take them.
    pub waiting: Vec<String>,
    /// The problem of the first cause; `None` exactly when `target` is `None`.
    pub worst: Option<WorstProblem>,
}

/// The most urgent problem behind an alarm, for the band of a wall display. It carries no
/// user names or comments, so a display token may read it.
#[derive(Serialize, PartialEq, Debug)]
pub struct WorstProblem {
    /// The name of the host.
    pub host: String,
    pub summary: String,
    pub severity: Severity,
    #[serde(serialize_with = "super::serialize_time")]
    pub started_at: OffsetDateTime,
}

impl DisplayTarget {
    fn none() -> Self {
        Self {
            target: None,
            causes: Vec::new(),
            waiting: Vec::new(),
            worst: None,
        }
    }
}

/// The dashboards the display may ever show: its `show` targets plus, when it switches on
/// alarm, its switch targets (the `dashboards` list, or every dashboard without one).
/// Sorted, without duplicates.
pub fn allowed_dashboards(display: &DisplayFile, all_dashboards: &[String]) -> Vec<String> {
    let mut allowed: BTreeSet<&str> = match &display.show {
        Show::Fixed { dashboard } => BTreeSet::from([dashboard.as_str()]),
        Show::Rotate { rotate, .. } => rotate.iter().map(String::as_str).collect(),
    };
    if display.switch_on_alarm.is_some() {
        match &display.dashboards {
            Some(list) => allowed.extend(list.iter().map(String::as_str)),
            None => allowed.extend(all_dashboards.iter().map(String::as_str)),
        }
    }
    allowed.into_iter().map(str::to_owned).collect()
}

/// An alarm for sorting: how urgent it is, then its id.
type Alarm = ((Reverse<Severity>, OffsetDateTime), Uuid);

/// The dashboard `display` switches to, the alarms that caused it and the dashboards that
/// wait behind it.
///
/// An alarm is an open problem nobody acknowledged, with a severity at or above
/// `min_severity`, on a host the dashboard selects. A dashboard with an alarm ranks by its
/// `priority` (higher first), then by its most urgent alarm (more severe, then older, the
/// same order as the start page uses), then by id.
pub fn alarm_target(display: &DisplayFile, inputs: &BoardInputs) -> DisplayTarget {
    let Some(switch) = &display.switch_on_alarm else {
        return DisplayTarget::none();
    };
    let mut ranked = Vec::new();
    for (id, _, dashboard) in &inputs.dashboards {
        if display
            .dashboards
            .as_ref()
            .is_some_and(|list| !list.contains(id))
        {
            continue;
        }
        let mut alarms: Vec<(Alarm, &Problem)> = selected_hosts(dashboard, &inputs.hosts)
            .into_iter()
            .flat_map(|h| h.open.iter())
            .filter(|p| p.acknowledged_at.is_none() && p.severity >= switch.min_severity)
            .map(|p| ((urgency(p.severity, p.started_at), p.id), p))
            .collect();
        alarms.sort_unstable_by_key(|(alarm, _)| *alarm);
        if let Some(((most_urgent, _), _)) = alarms.first().copied() {
            ranked.push(((Reverse(dashboard.priority), most_urgent, id), alarms));
        }
    }
    ranked.sort_by(|a, b| a.0.cmp(&b.0));
    let mut ranked = ranked.into_iter();
    let Some((first, alarms)) = ranked.next() else {
        return DisplayTarget::none();
    };
    let worst = alarms.first().map(|(_, p)| WorstProblem {
        host: p.host.clone(),
        summary: p.summary.clone(),
        severity: p.severity,
        started_at: p.started_at,
    });
    DisplayTarget {
        target: Some(first.2.clone()),
        causes: alarms.into_iter().map(|((_, id), _)| id).collect(),
        waiting: ranked.map(|(rank, _)| rank.2.clone()).collect(),
        worst,
    }
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use super::*;
    use crate::alerting::ProblemKind;
    use crate::state::HostRow;

    const T0: i64 = 1_800_000_000;

    fn problem(severity: Severity, age_secs: i64, acked: bool) -> Problem {
        let started =
            OffsetDateTime::from_unix_timestamp(T0).unwrap() - Duration::seconds(age_secs);
        Problem {
            id: Uuid::new_v4(),
            host_id: Uuid::nil(),
            host: String::new(),
            trigger: "T".into(),
            instance: String::new(),
            severity,
            kind: ProblemKind::Compare,
            summary: "s".into(),
            started_at: started,
            resolved_at: None,
            resolve_reason: None,
            acknowledged_at: acked.then_some(started),
            acknowledged_by: None,
            ack_comment: None,
            closed_by: None,
            close_comment: None,
        }
    }

    fn host(name: &str, groups: &[&str], open: Vec<Problem>) -> HostRow {
        HostRow {
            id: Uuid::new_v4(),
            name: name.into(),
            address: None,
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
            source_file: None,
            last_seen_at: None,
            open,
        }
    }

    /// `(id, priority, group)` per dashboard; a dashboard selects the hosts of its group.
    fn inputs(dashboards: &[(&str, i32, &str)], hosts: Vec<HostRow>) -> BoardInputs {
        BoardInputs {
            dashboards: dashboards
                .iter()
                .map(|(id, priority, group)| {
                    let json = format!(
                        r#"{{"name": "{id}", "priority": {priority}, "hosts": {{"groups": ["{group}"]}}}}"#
                    );
                    (
                        (*id).to_owned(),
                        format!("dashboards/{id}.json"),
                        serde_json::from_str(&json).unwrap(),
                    )
                })
                .collect(),
            hosts,
        }
    }

    fn display(json: &str) -> DisplayFile {
        serde_json::from_str(json).unwrap()
    }

    fn switching(min: &str) -> DisplayFile {
        display(&format!(
            r#"{{"name": "d", "show": {{"dashboard": "a"}}, "switch_on_alarm": {{"min_severity": "{min}"}}}}"#
        ))
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn no_switch_no_target() {
        let d = display(r#"{"name": "d", "show": {"dashboard": "a"}}"#);
        let i = inputs(
            &[("a", 0, "g")],
            vec![host(
                "h",
                &["g"],
                vec![problem(Severity::Critical, 5, false)],
            )],
        );
        assert_eq!(alarm_target(&d, &i), DisplayTarget::none());
    }

    #[test]
    fn highest_priority_wins() {
        let low = problem(Severity::Critical, 500, false);
        let high = problem(Severity::Critical, 5, false);
        let high_id = high.id;
        let i = inputs(
            &[("a", 1, "ga"), ("b", 9, "gb"), ("c", 5, "gc")],
            vec![
                host("h1", &["ga"], vec![low]),
                host("h2", &["gb"], vec![high]),
                host("h3", &["gc"], vec![]),
            ],
        );
        let t = alarm_target(&switching("critical"), &i);
        assert_eq!(t.target.as_deref(), Some("b"));
        assert_eq!(t.causes, vec![high_id]);
        assert_eq!(t.waiting, strings(&["a"]));
    }

    /// The open problems list shows the newest problem first; the alarm target is the other
    /// way round: of two equally severe alarms the older one wins.
    #[test]
    fn alarm_target_still_prefers_the_older_alarm() {
        let i = inputs(
            &[("new", 5, "gn"), ("old", 5, "go")],
            vec![
                host("h1", &["gn"], vec![problem(Severity::High, 10, false)]),
                host("h2", &["go"], vec![problem(Severity::High, 500, false)]),
            ],
        );
        let t = alarm_target(&switching("high"), &i);
        assert_eq!(t.target.as_deref(), Some("old"));
        assert_eq!(t.waiting, strings(&["new"]));
    }

    #[test]
    fn tie_goes_to_more_severe_then_older() {
        let i = inputs(
            &[("a", 5, "ga"), ("b", 5, "gb"), ("c", 5, "gc")],
            vec![
                host("h1", &["ga"], vec![problem(Severity::High, 900, false)]),
                host("h2", &["gb"], vec![problem(Severity::Critical, 10, false)]),
                host("h3", &["gc"], vec![problem(Severity::Critical, 60, false)]),
            ],
        );
        let t = alarm_target(&switching("high"), &i);
        assert_eq!(t.target.as_deref(), Some("c"));
        assert_eq!(t.waiting, strings(&["b", "a"]));
        // Equal in everything but the id: the id decides.
        let same = problem(Severity::Critical, 10, false);
        let mut twin = same.clone();
        twin.id = Uuid::new_v4();
        let i = inputs(
            &[("y", 5, "g"), ("x", 5, "g")],
            vec![host("h", &["g"], vec![same, twin])],
        );
        let t = alarm_target(&switching("critical"), &i);
        assert_eq!(t.target.as_deref(), Some("x"));
        assert_eq!(t.waiting, strings(&["y"]));
    }

    #[test]
    fn acknowledged_problems_are_not_alarms() {
        let i = inputs(
            &[("a", 0, "g")],
            vec![host(
                "h",
                &["g"],
                vec![problem(Severity::Critical, 5, true)],
            )],
        );
        assert_eq!(alarm_target(&switching("info"), &i).target, None);
    }

    #[test]
    fn below_min_severity_is_not_an_alarm() {
        let i = inputs(
            &[("a", 0, "g")],
            vec![host("h", &["g"], vec![problem(Severity::High, 5, false)])],
        );
        assert_eq!(alarm_target(&switching("critical"), &i).target, None);
        assert_eq!(
            alarm_target(&switching("high"), &i).target.as_deref(),
            Some("a")
        );
        assert_eq!(
            alarm_target(&switching("warning"), &i).target.as_deref(),
            Some("a")
        );
    }

    #[test]
    fn limited_display_never_targets_outside_its_list() {
        let d = display(
            r#"{"name": "d", "show": {"dashboard": "a"},
                "switch_on_alarm": {"min_severity": "info"}, "dashboards": ["a", "c"]}"#,
        );
        let i = inputs(
            &[("a", 0, "ga"), ("b", 99, "gb"), ("c", 5, "gc")],
            vec![
                host("h1", &["gb"], vec![problem(Severity::Critical, 5, false)]),
                host("h2", &["gc"], vec![problem(Severity::Info, 5, false)]),
            ],
        );
        let t = alarm_target(&d, &i);
        assert_eq!(t.target.as_deref(), Some("c"));
        assert!(t.waiting.is_empty());
    }

    #[test]
    fn host_in_two_dashboards_alarms_on_both() {
        let p = problem(Severity::Critical, 5, false);
        let id = p.id;
        let i = inputs(
            &[("a", 1, "g"), ("b", 2, "g")],
            vec![host("h", &["g"], vec![p])],
        );
        let t = alarm_target(&switching("critical"), &i);
        assert_eq!(t.target.as_deref(), Some("b"));
        assert_eq!(t.causes, vec![id]);
        assert_eq!(t.waiting, strings(&["a"]));
    }

    #[test]
    fn waiting_lists_the_rest_in_order() {
        let i = inputs(
            &[
                ("a", 1, "ga"),
                ("b", 4, "gb"),
                ("c", 3, "gc"),
                ("d", 2, "gd"),
            ],
            vec![
                host("h1", &["ga"], vec![problem(Severity::Critical, 5, false)]),
                host("h2", &["gb"], vec![problem(Severity::Critical, 5, false)]),
                host("h3", &["gc"], vec![problem(Severity::Critical, 5, false)]),
                host("h4", &["gd"], vec![]),
            ],
        );
        let t = alarm_target(&switching("critical"), &i);
        assert_eq!(t.target.as_deref(), Some("b"));
        assert_eq!(t.waiting, strings(&["c", "a"]));
    }

    #[test]
    fn causes_are_most_severe_first() {
        let old_high = problem(Severity::High, 900, false);
        let new_crit = problem(Severity::Critical, 5, false);
        let old_crit = problem(Severity::Critical, 50, false);
        let ids = vec![old_crit.id, new_crit.id, old_high.id];
        let i = inputs(
            &[("a", 0, "g")],
            vec![
                host("h1", &["g"], vec![old_high, new_crit]),
                host("h2", &["g"], vec![old_crit]),
            ],
        );
        assert_eq!(alarm_target(&switching("high"), &i).causes, ids);
    }

    fn named(host: &str, summary: &str, mut p: Problem) -> Problem {
        p.host = host.into();
        p.summary = summary.into();
        p
    }

    #[test]
    fn alarm_target_names_the_most_urgent_problem() {
        // The more severe one, although it is the younger one.
        let i = inputs(
            &[("a", 0, "g")],
            vec![
                host(
                    "web-02",
                    &["g"],
                    vec![named(
                        "web-02",
                        "Load high",
                        problem(Severity::High, 900, false),
                    )],
                ),
                host(
                    "web-01",
                    &["g"],
                    vec![named(
                        "web-01",
                        "Disk almost full",
                        problem(Severity::Critical, 5, false),
                    )],
                ),
            ],
        );
        let t = alarm_target(&switching("high"), &i);
        assert_eq!(
            t.worst,
            Some(WorstProblem {
                host: "web-01".into(),
                summary: "Disk almost full".into(),
                severity: Severity::Critical,
                started_at: OffsetDateTime::from_unix_timestamp(T0 - 5).unwrap(),
            })
        );
        // With equal severity, the older one.
        let i = inputs(
            &[("a", 0, "g")],
            vec![
                host(
                    "db-new",
                    &["g"],
                    vec![named("db-new", "Young", problem(Severity::High, 10, false))],
                ),
                host(
                    "db-old",
                    &["g"],
                    vec![named("db-old", "Old", problem(Severity::High, 600, false))],
                ),
            ],
        );
        let worst = alarm_target(&switching("high"), &i).worst.unwrap();
        assert_eq!(
            (worst.host.as_str(), worst.summary.as_str()),
            ("db-old", "Old")
        );
        assert_eq!(worst.severity, Severity::High);
        assert_eq!(
            worst.started_at,
            OffsetDateTime::from_unix_timestamp(T0 - 600).unwrap()
        );
        // The JSON form: severity as a word, the start as RFC 3339 in UTC.
        assert_eq!(
            serde_json::to_value(&worst).unwrap(),
            serde_json::json!({"host": "db-old", "summary": "Old", "severity": "high",
                               "started_at": "2027-01-15T07:50:00Z"})
        );
    }

    #[test]
    fn no_alarm_has_no_worst() {
        let i = inputs(
            &[("a", 0, "g")],
            vec![host(
                "h",
                &["g"],
                vec![problem(Severity::Critical, 5, true)],
            )],
        );
        let t = alarm_target(&switching("info"), &i);
        assert_eq!(t.target, None);
        assert_eq!(t.worst, None);
        assert_eq!(
            serde_json::to_value(&t).unwrap()["worst"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn allowed_is_show_plus_switch_targets() {
        let all = strings(&["a", "b", "c", "d"]);
        // Fixed, no switching: only what it shows.
        let d = display(r#"{"name": "d", "show": {"dashboard": "b"}}"#);
        assert_eq!(allowed_dashboards(&d, &all), strings(&["b"]));
        // Rotating, no switching: the rotation.
        let d = display(r#"{"name": "d", "show": {"rotate": ["c", "a", "c"], "every": "30s"}}"#);
        assert_eq!(allowed_dashboards(&d, &all), strings(&["a", "c"]));
        // Switching without a list: every dashboard.
        let d = display(r#"{"name": "d", "show": {"dashboard": "b"}, "switch_on_alarm": {}}"#);
        assert_eq!(allowed_dashboards(&d, &all), all);
        // Switching with a list: the list plus what it shows.
        let d = display(
            r#"{"name": "d", "show": {"rotate": ["a"], "every": "30s"},
                "switch_on_alarm": {}, "dashboards": ["a", "d"]}"#,
        );
        assert_eq!(allowed_dashboards(&d, &all), strings(&["a", "d"]));
        // A list without switching adds nothing beyond what is shown.
        let d = display(r#"{"name": "d", "show": {"dashboard": "a"}, "dashboards": ["a", "d"]}"#);
        assert_eq!(allowed_dashboards(&d, &all), strings(&["a"]));
    }

    #[test]
    fn deterministic_for_shuffled_input() {
        let mut hosts = vec![
            host("h1", &["ga"], vec![problem(Severity::Critical, 50, false)]),
            host("h2", &["gb"], vec![problem(Severity::Critical, 50, false)]),
            host("h3", &["gc"], vec![problem(Severity::High, 70, false)]),
            host("h4", &["gc"], vec![problem(Severity::Critical, 70, false)]),
        ];
        let boards = [
            ("a", 3, "ga"),
            ("b", 3, "gb"),
            ("c", 3, "gc"),
            ("d", 1, "gc"),
        ];
        let d = switching("high");
        let reference = alarm_target(&d, &inputs(&boards, hosts.clone()));
        assert_eq!(reference.target.as_deref(), Some("c"));
        assert_eq!(reference.waiting, strings(&["a", "b", "d"]));
        for round in 0..6 {
            hosts.rotate_left(1 + round % 3);
            let mut order = boards.to_vec();
            order.rotate_left(round % 4);
            order.reverse();
            let t = alarm_target(&d, &inputs(&order, hosts.clone()));
            assert_eq!(t, reference, "round {round}");
        }
    }
}
