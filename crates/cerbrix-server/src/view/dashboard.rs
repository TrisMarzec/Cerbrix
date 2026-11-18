//! Dashboards: which hosts a dashboard selects, what its start-page summary counts, and the
//! pure parts of the answer for one dashboard. The state of a host is [`host_state`], the
//! same rule as in the hosts list.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};

use cerbrix_common::DashboardFile;
use cerbrix_common::config::Severity;
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{HostState, host_state};
use crate::state::{BoardInputs, HostRow, Problem};

/// The problems panel shows at most this many problems.
pub const PROBLEMS_PANEL_LIMIT: usize = 100;

/// The hosts of `hosts` the dashboard selects, in their order. The one place that decides
/// which hosts a dashboard shows.
pub fn selected_hosts<'a>(d: &DashboardFile, hosts: &'a [HostRow]) -> Vec<&'a HostRow> {
    hosts
        .iter()
        .filter(|h| d.hosts.selects(&h.name, &h.groups))
        .collect()
}

/// A number per severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SeverityCounts {
    pub critical: usize,
    pub high: usize,
    pub warning: usize,
    pub info: usize,
}

impl SeverityCounts {
    fn add(&mut self, severity: Severity) {
        let slot = match severity {
            Severity::Critical => &mut self.critical,
            Severity::High => &mut self.high,
            Severity::Warning => &mut self.warning,
            Severity::Info => &mut self.info,
        };
        *slot += 1;
    }
}

/// The hosts of a dashboard by state. Every host is counted once: by the severity that
/// decides its state, unless all its open problems are acknowledged (`acknowledged`), or it
/// has none (`ok`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StateCounts {
    pub critical: usize,
    pub high: usize,
    pub warning: usize,
    pub info: usize,
    pub acknowledged: usize,
    pub ok: usize,
}

/// What the start page shows of a dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct DashboardSummary {
    pub id: String,
    pub name: String,
    pub file: String,
    pub priority: i32,
    pub host_count: usize,
    pub hosts_by_state: StateCounts,
    /// The open problems of the selected hosts per severity.
    pub open_problems: SeverityCounts,
    /// The open problems that nobody has acknowledged, per severity.
    pub unacknowledged: SeverityCounts,
}

/// How a host counts: from [`host_state`], so the rule stays in one place.
#[derive(Debug, Clone, Copy)]
enum Class {
    Ok,
    Acknowledged,
    Alarm(Severity),
}

fn class_of(host: &HostRow) -> Class {
    match host_state(&host.open) {
        None => Class::Ok,
        Some(state) if state.acknowledged => Class::Acknowledged,
        Some(state) => Class::Alarm(state.severity),
    }
}

/// The most severe unacknowledged problem of a dashboard, the oldest of those: its severity
/// and start.
type Urgent = Option<(Severity, OffsetDateTime)>;

/// How urgent a problem is, for sorting: the smaller, the more urgent. More severe first,
/// then the older. The start page and the alarm target of a wall display both order by this.
pub fn urgency(
    severity: Severity,
    started_at: OffsetDateTime,
) -> (Reverse<Severity>, OffsetDateTime) {
    (Reverse(severity), started_at)
}

/// The counts of the selected hosts and their most urgent unacknowledged problem.
fn count(
    hosts: &[&HostRow],
    classes: &HashMap<Uuid, Class>,
) -> (StateCounts, SeverityCounts, SeverityCounts, Urgent) {
    let mut by_state = StateCounts::default();
    let mut open = SeverityCounts::default();
    let mut unacknowledged = SeverityCounts::default();
    let mut urgent: Urgent = None;
    for host in hosts {
        match classes.get(&host.id).copied().unwrap_or(Class::Ok) {
            Class::Ok => by_state.ok += 1,
            Class::Acknowledged => by_state.acknowledged += 1,
            Class::Alarm(Severity::Critical) => by_state.critical += 1,
            Class::Alarm(Severity::High) => by_state.high += 1,
            Class::Alarm(Severity::Warning) => by_state.warning += 1,
            Class::Alarm(Severity::Info) => by_state.info += 1,
        }
        for p in &host.open {
            open.add(p.severity);
            if p.acknowledged_at.is_none() {
                unacknowledged.add(p.severity);
                let worse = urgent.is_none_or(|(severity, started)| {
                    urgency(p.severity, p.started_at) < urgency(severity, started)
                });
                if worse {
                    urgent = Some((p.severity, p.started_at));
                }
            }
        }
    }
    (by_state, open, unacknowledged, urgent)
}

/// The summaries of all dashboards, in the order of the start page: dashboards with an
/// unacknowledged open problem first (higher `priority` first, then the more severe
/// unacknowledged problem, then the older one), then the others by `priority` descending,
/// then name. Ties go by id, so the order is always the same.
pub fn summaries(inputs: &BoardInputs) -> Vec<DashboardSummary> {
    let classes: HashMap<Uuid, Class> = inputs.hosts.iter().map(|h| (h.id, class_of(h))).collect();
    let mut ranked: Vec<(DashboardSummary, Urgent)> = inputs
        .dashboards
        .iter()
        .map(|(id, file, dashboard)| {
            let selected = selected_hosts(dashboard, &inputs.hosts);
            let (hosts_by_state, open_problems, unacknowledged, urgent) =
                count(&selected, &classes);
            let summary = DashboardSummary {
                id: id.clone(),
                name: dashboard.name.clone(),
                file: file.clone(),
                priority: dashboard.priority,
                host_count: selected.len(),
                hosts_by_state,
                open_problems,
                unacknowledged,
            };
            (summary, urgent)
        })
        .collect();
    ranked.sort_by_cached_key(|(s, urgent)| {
        (
            urgent.is_none(),
            Reverse(s.priority),
            urgent.map(|(severity, started)| urgency(severity, started)),
            s.name.clone(),
            s.id.clone(),
        )
    });
    ranked.into_iter().map(|(summary, _)| summary).collect()
}

/// A host of a dashboard with its state.
#[derive(Debug, Serialize)]
pub struct HostEntry<'a> {
    pub name: &'a str,
    pub state: Option<HostState>,
}

/// The hosts with their state, problem hosts first: unacknowledged ones by severity, then
/// those whose problems are all acknowledged by severity, then the others; ties by name.
pub fn host_entries<'a>(hosts: &[&'a HostRow]) -> Vec<HostEntry<'a>> {
    let mut entries: Vec<_> = hosts
        .iter()
        .map(|h| {
            let state = host_state(&h.open);
            let rank = match &state {
                Some(s) if !s.acknowledged => (0, Some(Reverse(s.severity))),
                Some(s) => (1, Some(Reverse(s.severity))),
                None => (2, None),
            };
            (rank, h.name.as_str(), h.id, state)
        })
        .collect();
    entries.sort_by_key(|(rank, name, id, _)| (*rank, *name, *id));
    entries
        .into_iter()
        .map(|(_, name, _, state)| HostEntry { name, state })
        .collect()
}

/// A dashboard as a host's detail and the problems list name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DashboardRef {
    pub id: String,
    pub name: String,
}

/// The dashboards each of `hosts` belongs to, sorted by name then id; every host is a key,
/// with an empty list when it is in no dashboard. Membership is [`selected_hosts`].
pub fn dashboards_of_hosts(
    dashboards: &[(String, String, DashboardFile)],
    hosts: &[HostRow],
) -> BTreeMap<String, Vec<DashboardRef>> {
    let mut of: BTreeMap<String, Vec<DashboardRef>> =
        hosts.iter().map(|h| (h.name.clone(), Vec::new())).collect();
    for (id, _, dashboard) in dashboards {
        for host in selected_hosts(dashboard, hosts) {
            if let Some(list) = of.get_mut(&host.name) {
                list.push(DashboardRef {
                    id: id.clone(),
                    name: dashboard.name.clone(),
                });
            }
        }
    }
    for list in of.values_mut() {
        list.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
    }
    of
}

/// The open problems of `hosts`: unacknowledged first, most severe first, newest first, then
/// id. The same order as the open problems list (`problems_page`). It is not [`urgency`],
/// where the older alarm wins.
pub fn open_problems<'a>(hosts: &[&'a HostRow]) -> Vec<&'a Problem> {
    let mut problems: Vec<&Problem> = hosts.iter().flat_map(|h| h.open.iter()).collect();
    problems.sort_by_key(|p| {
        (
            p.acknowledged_at.is_some(),
            Reverse(p.severity),
            Reverse(p.started_at),
            p.id,
        )
    });
    problems
}

#[cfg(test)]
mod tests {
    use cerbrix_common::DashboardFile;
    use cerbrix_common::config::Severity;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::*;
    use crate::alerting::ProblemKind;
    use crate::state::{BoardInputs, HostRow, Problem};

    const T0: i64 = 1_800_000_000;

    fn problem(
        host: &str,
        trigger: &str,
        severity: Severity,
        age_secs: i64,
        acked: bool,
    ) -> Problem {
        let started =
            OffsetDateTime::from_unix_timestamp(T0).unwrap() - Duration::seconds(age_secs);
        Problem {
            id: Uuid::new_v4(),
            host_id: Uuid::nil(),
            host: host.into(),
            trigger: trigger.into(),
            instance: String::new(),
            severity,
            kind: ProblemKind::Compare,
            summary: format!("{trigger} summary"),
            started_at: started,
            resolved_at: None,
            resolve_reason: None,
            acknowledged_at: acked.then_some(started),
            acknowledged_by: acked.then(|| "anna".to_owned()),
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

    fn dashboard(json: &str) -> DashboardFile {
        serde_json::from_str(json).unwrap()
    }

    fn inputs(dashboards: &[(&str, &str)], hosts: Vec<HostRow>) -> BoardInputs {
        BoardInputs {
            dashboards: dashboards
                .iter()
                .map(|(id, json)| {
                    (
                        (*id).to_owned(),
                        format!("dashboards/{id}.json"),
                        dashboard(json),
                    )
                })
                .collect(),
            hosts,
        }
    }

    fn names(hosts: &[&HostRow]) -> Vec<String> {
        hosts.iter().map(|h| h.name.clone()).collect()
    }

    #[test]
    fn summaries_count_hosts_and_problems() {
        let hosts = vec![
            // Unacknowledged critical and acknowledged high: the host is a critical alarm.
            host(
                "a",
                &["prod"],
                vec![
                    problem("a", "Crit", Severity::Critical, 50, false),
                    problem("a", "High", Severity::High, 60, true),
                ],
            ),
            // Only acknowledged problems: counted as acknowledged, not by severity.
            host(
                "b",
                &["prod"],
                vec![problem("b", "W", Severity::Warning, 10, true)],
            ),
            host("c", &["prod"], vec![]),
            // An unacknowledged info problem makes the host an info alarm.
            host(
                "d",
                &["prod"],
                vec![problem("d", "I", Severity::Info, 10, false)],
            ),
            // Not selected.
            host(
                "e",
                &["other"],
                vec![problem("e", "X", Severity::Critical, 10, false)],
            ),
        ];
        let inputs = inputs(
            &[(
                "prod",
                r#"{"name":"Prod","priority":4,"hosts":{"groups":["prod"]}}"#,
            )],
            hosts,
        );
        let all = summaries(&inputs);
        assert_eq!(all.len(), 1);
        let s = &all[0];
        assert_eq!(
            (
                s.id.as_str(),
                s.name.as_str(),
                s.file.as_str(),
                s.priority,
                s.host_count
            ),
            ("prod", "Prod", "dashboards/prod.json", 4, 4)
        );
        assert_eq!(
            serde_json::to_value(s.hosts_by_state).unwrap(),
            serde_json::json!({
                "critical": 1, "high": 0, "warning": 0, "info": 1, "acknowledged": 1, "ok": 1
            })
        );
        assert_eq!(
            serde_json::to_value(s.open_problems).unwrap(),
            serde_json::json!({"critical": 1, "high": 1, "warning": 1, "info": 1})
        );
        assert_eq!(
            serde_json::to_value(s.unacknowledged).unwrap(),
            serde_json::json!({"critical": 1, "high": 0, "warning": 0, "info": 1})
        );
    }

    #[test]
    fn summaries_order_follows_alarms_then_priority() {
        // One host per dashboard, so each dashboard has exactly the alarm named here.
        type Alarm = Option<(Severity, i64, bool)>;
        let spec: [(&str, &str, i32, Alarm); 9] = [
            ("quiet-hi", "Quiet high", 9, None),
            ("quiet-b", "Quiet a", 1, None),
            ("quiet-a", "Quiet b", 1, None),
            // An acknowledged problem is no alarm.
            ("acked", "Acked", 100, Some((Severity::Critical, 10, true))),
            (
                "alarm-lo",
                "Alarm low",
                1,
                Some((Severity::Critical, 10, false)),
            ),
            (
                "alarm-info",
                "Alarm info",
                5,
                Some((Severity::Info, 10, false)),
            ),
            (
                "alarm-warn",
                "Alarm warning",
                5,
                Some((Severity::Warning, 10, false)),
            ),
            (
                "alarm-new",
                "Alarm new",
                5,
                Some((Severity::High, 10, false)),
            ),
            (
                "alarm-old",
                "Alarm old",
                5,
                Some((Severity::High, 500, false)),
            ),
        ];
        let mut hosts = Vec::new();
        let mut dashboards = Vec::new();
        for (id, name, priority, alarm) in &spec {
            let open = alarm
                .map(|(sev, age, acked)| vec![problem(id, "T", sev, age, acked)])
                .unwrap_or_default();
            hosts.push(host(id, &[], open));
            dashboards.push((
                (*id).to_owned(),
                format!(
                    r#"{{"name":"{name}","priority":{priority},"hosts":{{"names":["{id}"]}}}}"#
                ),
            ));
        }
        let refs: Vec<(&str, &str)> = dashboards
            .iter()
            .map(|(i, j)| (i.as_str(), j.as_str()))
            .collect();
        let order: Vec<String> = summaries(&inputs(&refs, hosts))
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(
            order,
            [
                // Alarms: priority 5 before 1; among priority 5 the more severe, then older.
                "alarm-old",
                "alarm-new",
                "alarm-warn",
                "alarm-info",
                "alarm-lo",
                // The rest: priority, then name (not id).
                "acked",
                "quiet-hi",
                "quiet-b",
                "quiet-a",
            ]
        );
    }

    #[test]
    fn a_dashboard_with_an_alarm_comes_before_a_higher_priority_without_one() {
        let hosts = vec![
            host(
                "loud",
                &[],
                vec![problem("loud", "T", Severity::Info, 1, false)],
            ),
            host("calm", &[], vec![]),
        ];
        let order: Vec<String> = summaries(&inputs(
            &[
                (
                    "a",
                    r#"{"name":"A","priority":100,"hosts":{"names":["calm"]}}"#,
                ),
                (
                    "b",
                    r#"{"name":"B","priority":-5,"hosts":{"names":["loud"]}}"#,
                ),
            ],
            hosts,
        ))
        .into_iter()
        .map(|s| s.id)
        .collect();
        assert_eq!(order, ["b", "a"]);
    }

    #[test]
    fn selector_semantics_reused() {
        let hosts = vec![
            host("web-1", &["prod", "web"], vec![]),
            host("web-2", &["web"], vec![]),
            host("db-1", &["prod", "db"], vec![]),
            host("lone", &[], vec![]),
        ];
        // A host must be in all the listed groups.
        let both = dashboard(r#"{"name":"d","hosts":{"groups":["prod","web"]}}"#);
        assert_eq!(names(&selected_hosts(&both, &hosts)), ["web-1"]);
        // A named host is added to the group match.
        let named = dashboard(r#"{"name":"d","hosts":{"groups":["db"],"names":["lone"]}}"#);
        assert_eq!(names(&selected_hosts(&named, &hosts)), ["db-1", "lone"]);
        // The empty selector, and a missing one, select every host.
        for json in [r#"{"name":"d"}"#, r#"{"name":"d","hosts":{}}"#] {
            let all = dashboard(json);
            assert_eq!(selected_hosts(&all, &hosts).len(), 4, "{json}");
        }
        // The order of the input is kept.
        let some = dashboard(r#"{"name":"d","hosts":{"names":["lone","web-2"]}}"#);
        assert_eq!(names(&selected_hosts(&some, &hosts)), ["web-2", "lone"]);
    }

    #[test]
    fn host_entries_list_problem_hosts_first() {
        let hosts = [
            host("a-ok", &[], vec![]),
            host(
                "b-acked-crit",
                &[],
                vec![problem("b", "T", Severity::Critical, 5, true)],
            ),
            host(
                "c-warn",
                &[],
                vec![problem("c", "T", Severity::Warning, 5, false)],
            ),
            host(
                "d-crit",
                &[],
                vec![problem("d", "T", Severity::Critical, 5, false)],
            ),
            host(
                "e-acked-info",
                &[],
                vec![problem("e", "T", Severity::Info, 5, true)],
            ),
            host(
                "f-crit",
                &[],
                vec![problem("f", "T", Severity::Critical, 5, false)],
            ),
            host("g-ok", &[], vec![]),
        ];
        let refs: Vec<&HostRow> = hosts.iter().collect();
        let entries = host_entries(&refs);
        let order: Vec<&str> = entries.iter().map(|e| e.name).collect();
        assert_eq!(
            order,
            [
                "d-crit",
                "f-crit",
                "c-warn",
                "b-acked-crit",
                "e-acked-info",
                "a-ok",
                "g-ok"
            ]
        );
        assert!(entries[0].state.is_some() && entries[5].state.is_none());
        let json = serde_json::to_value(&entries[0]).unwrap();
        assert_eq!(json["name"], "d-crit");
        assert_eq!(json["state"]["severity"], "critical");
    }

    #[test]
    fn open_problems_are_newest_first_within_a_severity() {
        let hosts = [
            host(
                "a",
                &[],
                vec![
                    problem("a", "acked-crit", Severity::Critical, 900, true),
                    problem("a", "new-high", Severity::High, 10, false),
                ],
            ),
            host(
                "b",
                &[],
                vec![
                    problem("b", "old-high", Severity::High, 500, false),
                    problem("b", "crit", Severity::Critical, 1, false),
                    problem("b", "acked-info", Severity::Info, 800, true),
                ],
            ),
        ];
        let refs: Vec<&HostRow> = hosts.iter().collect();
        let order: Vec<&str> = open_problems(&refs)
            .iter()
            .map(|p| p.trigger.as_str())
            .collect();
        assert_eq!(
            order,
            ["crit", "new-high", "old-high", "acked-crit", "acked-info"]
        );
    }

    #[test]
    fn counts_are_right_for_many_hosts() {
        let mut hosts = Vec::new();
        for i in 0..1000 {
            let name = format!("h{i:04}");
            let open = match i % 4 {
                // Two unacknowledged problems: critical and warning.
                0 => vec![
                    problem(&name, "A", Severity::Critical, 5, false),
                    problem(&name, "B", Severity::Warning, 5, false),
                ],
                // Two acknowledged problems.
                1 => vec![
                    problem(&name, "A", Severity::High, 5, true),
                    problem(&name, "B", Severity::Info, 5, true),
                ],
                // One of each: the unacknowledged one decides.
                2 => vec![
                    problem(&name, "A", Severity::Critical, 5, true),
                    problem(&name, "B", Severity::High, 5, false),
                ],
                _ => vec![],
            };
            hosts.push(host(&name, &["g"], open));
        }
        let all = summaries(&inputs(&[("all", r#"{"name":"All"}"#)], hosts));
        let s = &all[0];
        assert_eq!(s.host_count, 1000);
        assert_eq!(
            serde_json::to_value(s.hosts_by_state).unwrap(),
            serde_json::json!({
                "critical": 250, "high": 250, "warning": 0, "info": 0,
                "acknowledged": 250, "ok": 250
            })
        );
        assert_eq!(
            serde_json::to_value(s.open_problems).unwrap(),
            serde_json::json!({"critical": 500, "high": 500, "warning": 250, "info": 250})
        );
        assert_eq!(
            serde_json::to_value(s.unacknowledged).unwrap(),
            serde_json::json!({"critical": 250, "high": 250, "warning": 250, "info": 0})
        );
        let hosts_total: usize = {
            let h = &s.hosts_by_state;
            h.critical + h.high + h.warning + h.info + h.acknowledged + h.ok
        };
        assert_eq!(hosts_total, 1000, "every host is counted once");
    }
}
