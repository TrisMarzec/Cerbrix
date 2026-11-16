//! What the web UI shows about the state of things, derived from stored data.
//!
//! The one rule for a host's state lives here: the main list, the dashboards and the wall
//! displays all show [`host_state`].

pub mod dashboard;
pub mod display;
pub mod series;

use std::cmp::Reverse;

use cerbrix_common::config::Severity;
use serde::{Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::alerting::ProblemKind;
use crate::state::Problem;

/// The state of a host with open problems: the problem that decides it, and how many are
/// open.
#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct HostState {
    pub severity: Severity,
    /// True only when every open problem of the host is acknowledged.
    pub acknowledged: bool,
    pub kind: ProblemKind,
    pub trigger: String,
    pub instance: String,
    pub summary: String,
    pub open_problems: usize,
}

/// The state of a host from its open problems; `None` when it has none.
///
/// The problem that decides: an unacknowledged one outranks an acknowledged one; then the
/// higher severity; then the older one; remaining ties go by trigger name, then instance.
pub fn host_state(open_problems_of_host: &[Problem]) -> Option<HostState> {
    let key = |p: &Problem| {
        (
            p.acknowledged_at.is_some(),
            Reverse(p.severity),
            p.started_at,
            p.trigger.clone(),
            p.instance.clone(),
        )
    };
    let deciding = open_problems_of_host.iter().min_by_key(|p| key(p))?;
    Some(HostState {
        severity: deciding.severity,
        acknowledged: open_problems_of_host
            .iter()
            .all(|p| p.acknowledged_at.is_some()),
        kind: deciding.kind,
        trigger: deciding.trigger.clone(),
        instance: deciding.instance.clone(),
        summary: deciding.summary.clone(),
        open_problems: open_problems_of_host.len(),
    })
}

/// A time as RFC 3339 in UTC, the form of every time in the API.
pub fn rfc3339(t: OffsetDateTime) -> String {
    t.to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// `Option<OffsetDateTime>` as an RFC 3339 string or `null`.
pub fn serialize_time_opt<S: Serializer>(
    t: &Option<OffsetDateTime>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match t {
        Some(t) => s.serialize_str(&rfc3339(*t)),
        None => s.serialize_none(),
    }
}

/// The JSON form of a problem, defined once for every route that shows one.
impl Serialize for Problem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Out<'a> {
            id: String,
            host: &'a str,
            trigger: &'a str,
            instance: &'a str,
            severity: Severity,
            kind: ProblemKind,
            summary: &'a str,
            started_at: String,
            #[serde(serialize_with = "serialize_time_opt")]
            resolved_at: Option<OffsetDateTime>,
            resolve_reason: &'a Option<String>,
            #[serde(serialize_with = "serialize_time_opt")]
            acknowledged_at: Option<OffsetDateTime>,
            acknowledged_by: &'a Option<String>,
            ack_comment: &'a Option<String>,
            closed_by: &'a Option<String>,
            close_comment: &'a Option<String>,
        }
        Out {
            id: self.id.to_string(),
            host: &self.host,
            trigger: &self.trigger,
            instance: &self.instance,
            severity: self.severity,
            kind: self.kind,
            summary: &self.summary,
            started_at: rfc3339(self.started_at),
            resolved_at: self.resolved_at,
            resolve_reason: &self.resolve_reason,
            acknowledged_at: self.acknowledged_at,
            acknowledged_by: &self.acknowledged_by,
            ack_comment: &self.ack_comment,
            closed_by: &self.closed_by,
            close_comment: &self.close_comment,
        }
        .serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use time::Duration;
    use uuid::Uuid;

    use super::*;

    const T0: i64 = 1_800_000_000;

    fn problem(trigger: &str, severity: Severity, age_secs: i64, acked: bool) -> Problem {
        let started =
            OffsetDateTime::from_unix_timestamp(T0).unwrap() - Duration::seconds(age_secs);
        Problem {
            id: Uuid::new_v4(),
            host_id: Uuid::nil(),
            host: "web-01".into(),
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

    #[test]
    fn host_state_rule() {
        assert_eq!(host_state(&[]), None);

        // One problem: it decides, and a lone acknowledged one makes the host acknowledged.
        let one = host_state(&[problem("A", Severity::High, 10, true)]).unwrap();
        assert_eq!((one.trigger.as_str(), one.acknowledged), ("A", true));
        assert_eq!(one.open_problems, 1);

        // An unacknowledged warning beats an acknowledged critical; the host is not
        // acknowledged because one problem is not.
        let s = host_state(&[
            problem("Crit", Severity::Critical, 100, true),
            problem("Warn", Severity::Warning, 5, false),
        ])
        .unwrap();
        assert_eq!(s.trigger, "Warn");
        assert_eq!(s.severity, Severity::Warning);
        assert!(!s.acknowledged);
        assert_eq!(s.open_problems, 2);

        // Among unacknowledged ones, critical beats high whatever their age.
        let s = host_state(&[
            problem("High", Severity::High, 1000, false),
            problem("Crit", Severity::Critical, 1, false),
        ])
        .unwrap();
        assert_eq!(s.trigger, "Crit");

        // Equal severity: the older one.
        let s = host_state(&[
            problem("Newer", Severity::High, 10, false),
            problem("Older", Severity::High, 500, false),
        ])
        .unwrap();
        assert_eq!(s.trigger, "Older");

        // Only acknowledged problems: the rule applies among them, and the host is
        // acknowledged.
        let s = host_state(&[
            problem("Info", Severity::Info, 900, true),
            problem("High", Severity::High, 1, true),
        ])
        .unwrap();
        assert_eq!(s.trigger, "High");
        assert!(s.acknowledged);

        // A full tie is broken by trigger name, then instance, whatever the input order.
        let mut a = problem("Same", Severity::High, 10, false);
        a.instance = "mount=/".into();
        let mut b = a.clone();
        b.instance = "mount=/a".into();
        for list in [vec![a.clone(), b.clone()], vec![b, a.clone()]] {
            assert_eq!(host_state(&list).unwrap().instance, "mount=/");
        }
        let mut zed = a.clone();
        zed.trigger = "Zed".into();
        let mut alpha = a;
        alpha.trigger = "Alpha".into();
        assert_eq!(host_state(&[zed, alpha]).unwrap().trigger, "Alpha");
    }

    #[test]
    fn host_state_reports_the_kind_of_the_deciding_problem() {
        let mut nodata = problem("Silent", Severity::Critical, 10, false);
        nodata.kind = ProblemKind::NoData;
        let s = host_state(&[problem("Cpu", Severity::Warning, 10, false), nodata]).unwrap();
        assert_eq!(s.kind, ProblemKind::NoData);
        assert_eq!(serde_json::to_value(&s).unwrap()["kind"], "no_data");
    }

    #[test]
    fn problem_json_shape() {
        let mut p = problem("Disk", Severity::High, 0, true);
        p.ack_comment = Some("looking".into());
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["host"], "web-01");
        assert_eq!(v["severity"], "high");
        assert_eq!(v["kind"], "compare");
        assert_eq!(v["started_at"], "2027-01-15T08:00:00Z");
        assert_eq!(v["acknowledged_at"], "2027-01-15T08:00:00Z");
        assert_eq!(v["acknowledged_by"], "anna");
        assert_eq!(v["ack_comment"], "looking");
        assert!(v["resolved_at"].is_null());
        assert!(v["resolve_reason"].is_null());
        assert!(v["closed_by"].is_null() && v["close_comment"].is_null());
        assert_eq!(v.as_object().unwrap().len(), 15);
    }
}
