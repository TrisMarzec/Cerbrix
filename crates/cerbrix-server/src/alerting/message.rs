//! The notification about a problem and its plain-text subject and body.

use cerbrix_common::config::Severity;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Longest subject, in characters.
const MAX_SUBJECT_CHARS: usize = 200;

/// Whether a problem started or was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Started,
    Resolved,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Started => "started",
            EventKind::Resolved => "resolved",
        }
    }
}

/// One e-mail about a problem.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub kind: EventKind,
    pub host: String,
    pub trigger: String,
    /// `""` for no parameters, otherwise `k=v` pairs joined by `,`.
    pub instance: String,
    pub severity: Severity,
    pub summary: String,
    pub started_at: OffsetDateTime,
    pub resolved_at: Option<OffsetDateTime>,
    /// `condition_ended`, `host_removed` or `trigger_removed`.
    pub resolve_reason: Option<String>,
}

impl Notification {
    /// The subject line (host and trigger name): no control characters, at most 200 characters.
    pub fn subject(&self) -> String {
        let host = clean(&self.host);
        let trigger = clean(&self.trigger);
        let instance = clean(&self.instance);
        let head = match self.kind {
            EventKind::Started => severity_word(self.severity),
            EventKind::Resolved => "Resolved",
        };
        let mut subject = format!("[Cerbrix] {head} {host}: {trigger}");
        if !instance.is_empty() {
            subject.push_str(&format!(" ({instance})"));
        }
        subject.chars().take(MAX_SUBJECT_CHARS).collect()
    }

    /// The plain-text body: `Label: value` lines, each ending in `\n`.
    pub fn body(&self) -> String {
        let instance = clean(&self.instance);
        let state = match self.kind {
            EventKind::Started => "problem",
            EventKind::Resolved => "resolved",
        };
        let mut lines = vec![
            format!("Host: {}", clean(&self.host)),
            format!("Trigger: {}", clean(&self.trigger)),
        ];
        if !instance.is_empty() {
            lines.push(format!("Instance: {instance}"));
        }
        lines.push(format!("Severity: {}", severity_word(self.severity)));
        lines.push(format!("State: {state}"));
        lines.push(format!("Detail: {}", clean(&self.summary)));
        lines.push(format!("Started: {}", rfc3339(self.started_at)));
        if self.kind == EventKind::Resolved {
            if let Some(end) = self.resolved_at {
                lines.push(format!("Ended: {}", rfc3339(end)));
                lines.push(format!(
                    "Duration: {}",
                    duration_text((end - self.started_at).whole_seconds())
                ));
            }
            if let Some(reason) = &self.resolve_reason {
                lines.push(format!("Reason: {}", reason_text(reason)));
            }
        }
        let mut body = lines.join("\n");
        body.push_str("\n\nSent by Cerbrix.\n");
        body
    }
}

pub(crate) fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "INFO",
        Severity::Warning => "WARNING",
        Severity::High => "HIGH",
        Severity::Critical => "CRITICAL",
    }
}

/// Replaces every control character by a space, so no field can add lines or headers.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub(crate) fn reason_text(reason: &str) -> String {
    match reason {
        "condition_ended" => "the condition no longer holds".to_string(),
        "host_removed" => "the host was removed from the configuration".to_string(),
        "trigger_removed" => "the trigger was removed from the configuration".to_string(),
        other => clean(other),
    }
}

/// RFC 3339 in UTC with seconds precision, e.g. `2026-11-15T14:20:00Z`.
pub(crate) fn rfc3339(at: OffsetDateTime) -> String {
    let at = at.to_offset(time::UtcOffset::UTC);
    let at = at.replace_nanosecond(0).unwrap_or(at);
    at.format(&Rfc3339).unwrap_or_else(|_| at.to_string())
}

/// The two largest non-zero units out of d, h, min, s; `0 s` for zero or negative.
fn duration_text(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let units = [
        (seconds / 86_400, "d"),
        (seconds % 86_400 / 3_600, "h"),
        (seconds % 3_600 / 60, "min"),
        (seconds % 60, "s"),
    ];
    let parts: Vec<String> = units
        .iter()
        .filter(|(n, _)| *n > 0)
        .take(2)
        .map(|(n, u)| format!("{n} {u}"))
        .collect();
    if parts.is_empty() {
        "0 s".to_string()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(unix: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(unix).unwrap()
    }

    // 2026-11-15T14:20:00Z
    const T0: i64 = 1_794_752_400;

    fn started() -> Notification {
        Notification {
            kind: EventKind::Started,
            host: "db-02".to_string(),
            trigger: "Disk almost full".to_string(),
            instance: "mount=/".to_string(),
            severity: Severity::High,
            summary: "disk.used_percent > 90 for 10m: value 96.5 on mount=/".to_string(),
            started_at: at(T0),
            resolved_at: None,
            resolve_reason: None,
        }
    }

    fn resolved(seconds: i64) -> Notification {
        Notification {
            kind: EventKind::Resolved,
            resolved_at: Some(at(T0 + seconds)),
            resolve_reason: Some("condition_ended".to_string()),
            ..started()
        }
    }

    #[test]
    fn subject_variants() {
        let critical = Notification {
            severity: Severity::Critical,
            instance: String::new(),
            ..started()
        };
        assert_eq!(
            critical.subject(),
            "[Cerbrix] CRITICAL db-02: Disk almost full"
        );
        assert_eq!(
            started().subject(),
            "[Cerbrix] HIGH db-02: Disk almost full (mount=/)"
        );
        assert_eq!(
            resolved(60).subject(),
            "[Cerbrix] Resolved db-02: Disk almost full (mount=/)"
        );
    }

    #[test]
    fn event_kind_text() {
        assert_eq!(EventKind::Started.as_str(), "started");
        assert_eq!(EventKind::Resolved.as_str(), "resolved");
    }

    #[test]
    fn body_started() {
        assert_eq!(
            started().body(),
            "Host: db-02\n\
             Trigger: Disk almost full\n\
             Instance: mount=/\n\
             Severity: HIGH\n\
             State: problem\n\
             Detail: disk.used_percent > 90 for 10m: value 96.5 on mount=/\n\
             Started: 2026-11-15T14:20:00Z\n\
             \n\
             Sent by Cerbrix.\n"
        );
        let no_instance = Notification {
            instance: String::new(),
            ..started()
        };
        assert!(!no_instance.body().contains("Instance:"));
    }

    #[test]
    fn body_resolved_has_duration_and_reason() {
        assert_eq!(
            resolved(3_900).body(),
            "Host: db-02\n\
             Trigger: Disk almost full\n\
             Instance: mount=/\n\
             Severity: HIGH\n\
             State: resolved\n\
             Detail: disk.used_percent > 90 for 10m: value 96.5 on mount=/\n\
             Started: 2026-11-15T14:20:00Z\n\
             Ended: 2026-11-15T15:25:00Z\n\
             Duration: 1 h 5 min\n\
             Reason: the condition no longer holds\n\
             \n\
             Sent by Cerbrix.\n"
        );
        for (seconds, text) in [
            (45, "45 s"),
            (720, "12 min"),
            (3_900, "1 h 5 min"),
            (3_600, "1 h"),
            (183_600, "2 d 3 h"),
            (0, "0 s"),
            (-30, "0 s"),
        ] {
            assert!(
                resolved(seconds)
                    .body()
                    .contains(&format!("Duration: {text}\n")),
                "{seconds} s should read {text}"
            );
        }
    }

    #[test]
    fn reasons_and_unknown_reason() {
        for (reason, text) in [
            (
                "host_removed",
                "the host was removed from the configuration",
            ),
            (
                "trigger_removed",
                "the trigger was removed from the configuration",
            ),
            ("other\nthing", "other thing"),
        ] {
            let n = Notification {
                resolve_reason: Some(reason.to_string()),
                ..resolved(60)
            };
            assert!(n.body().contains(&format!("Reason: {text}\n")));
        }
        let none = Notification {
            resolve_reason: None,
            ..resolved(60)
        };
        assert!(!none.body().contains("Reason:"));
    }

    #[test]
    fn rfc3339_is_utc_without_subseconds() {
        let n = Notification {
            started_at: at(T0)
                .replace_nanosecond(123_000_000)
                .unwrap()
                .to_offset(time::UtcOffset::from_hms(2, 0, 0).unwrap()),
            ..started()
        };
        assert!(n.body().contains("Started: 2026-11-15T14:20:00Z\n"));
    }

    #[test]
    fn no_line_breaks_from_fields() {
        let n = Notification {
            summary: "x\r\nBcc: x@y".to_string(),
            host: "h\nost".to_string(),
            trigger: "t\rrig".to_string(),
            instance: "k=v\n".to_string(),
            ..started()
        };
        let subject = n.subject();
        assert!(!subject.contains('\r') && !subject.contains('\n'));
        assert_eq!(subject, "[Cerbrix] HIGH h ost: t rig (k=v )");
        let body = n.body();
        assert!(body.contains("Detail: x  Bcc: x@y\n"));
        assert!(body.contains("Host: h ost\n"));
        assert!(body.contains("Trigger: t rig\n"));
        assert!(body.contains("Instance: k=v \n"));
        assert!(!body.contains('\r'));
        assert_eq!(body.lines().count(), 9);
    }

    #[test]
    fn subject_is_cut_to_200_characters() {
        let n = Notification {
            trigger: "\u{e4}".repeat(500),
            ..started()
        };
        assert_eq!(n.subject().chars().count(), 200);
    }
}
