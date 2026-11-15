//! Which e-mail addresses a problem is sent to.

use std::collections::BTreeSet;

use cerbrix_common::NotificationFile;
use cerbrix_common::config::Severity;

/// Addresses of every rule that matches, without duplicates, sorted.
///
/// A rule matches when `severity >= rule.min_severity` and the host is in every
/// group of `rule.groups` (a rule without groups matches every host). Addresses
/// are compared as written, case-sensitively.
pub fn recipients(
    rules: &[NotificationFile],
    severity: Severity,
    host_groups: &[String],
) -> Vec<String> {
    rules
        .iter()
        .filter(|r| severity >= r.min_severity && r.groups.iter().all(|g| host_groups.contains(g)))
        .flat_map(|r| r.email.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(emails: &[&str], min: Severity, groups: &[&str]) -> NotificationFile {
        NotificationFile {
            name: "r".to_string(),
            email: emails.iter().map(|s| s.to_string()).collect(),
            min_severity: min,
            groups: groups.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn groups(g: &[&str]) -> Vec<String> {
        g.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn matches_min_severity() {
        let rules = [rule(&["a@x.org"], Severity::High, &[])];
        assert_eq!(recipients(&rules, Severity::Critical, &[]), ["a@x.org"]);
        assert_eq!(recipients(&rules, Severity::High, &[]), ["a@x.org"]);
        assert!(recipients(&rules, Severity::Warning, &[]).is_empty());
    }

    #[test]
    fn matches_all_groups() {
        let rules = [rule(&["a@x.org"], Severity::Info, &["production", "db"])];
        assert_eq!(
            recipients(&rules, Severity::Info, &groups(&["db", "production", "eu"])),
            ["a@x.org"]
        );
        assert!(recipients(&rules, Severity::Info, &groups(&["production"])).is_empty());
        assert!(recipients(&rules, Severity::Info, &[]).is_empty());

        let all = [rule(&["b@x.org"], Severity::Info, &[])];
        assert_eq!(recipients(&all, Severity::Info, &[]), ["b@x.org"]);
        assert_eq!(
            recipients(&all, Severity::Info, &groups(&["x"])),
            ["b@x.org"]
        );
    }

    #[test]
    fn recipients_are_unique_and_sorted() {
        let rules = [
            rule(&["zed@x.org", "shared@x.org"], Severity::Info, &[]),
            rule(&["shared@x.org", "alpha@x.org"], Severity::Warning, &[]),
        ];
        assert_eq!(
            recipients(&rules, Severity::High, &[]),
            ["alpha@x.org", "shared@x.org", "zed@x.org"]
        );
    }

    #[test]
    fn no_rule_no_recipient() {
        assert!(recipients(&[], Severity::Critical, &[]).is_empty());
    }
}
