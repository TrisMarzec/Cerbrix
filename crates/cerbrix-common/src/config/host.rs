//! Host, template and notification files.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Severity;
use crate::interval::Interval;

/// An item a host or template collects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemEntry {
    pub key: String,
    pub params: Option<BTreeMap<String, String>>,
    pub interval: Interval,
}

/// A condition that opens a problem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerEntry {
    pub name: String,
    pub expression: String,
    pub severity: Severity,
}

/// A file in `hosts/`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostFile {
    pub host: String,
    pub address: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub templates: Vec<String>,
    #[serde(default)]
    pub items: Vec<ItemEntry>,
    #[serde(default)]
    pub triggers: Vec<TriggerEntry>,
}

/// A file in `templates/`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateFile {
    pub template: String,
    #[serde(default)]
    pub items: Vec<ItemEntry>,
    #[serde(default)]
    pub triggers: Vec<TriggerEntry>,
}

/// A file in `notifications/`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationFile {
    pub name: String,
    pub email: Vec<String>,
    pub min_severity: Severity,
    #[serde(default)]
    pub groups: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_EXAMPLE: &str = r#"{
  "host": "web-01",
  "address": "10.0.4.12",
  "groups": ["production", "webserver"],
  "templates": ["linux-base"],
  "items": [
    { "key": "cpu.usage", "interval": "30s" },
    { "key": "disk.used_percent", "params": { "mount": "/" }, "interval": "5m" }
  ],
  "triggers": [
    {
      "name": "Disk almost full",
      "expression": "disk.used_percent{mount=/} > 90 for 10m",
      "severity": "high"
    }
  ]
}"#;

    #[test]
    fn host_file_parses_spec_example() {
        let h: HostFile = serde_json::from_str(SPEC_EXAMPLE).unwrap();
        assert_eq!(h.host, "web-01");
        assert_eq!(h.address.as_deref(), Some("10.0.4.12"));
        assert_eq!(h.templates, ["linux-base"]);
        assert_eq!(
            h.items[1].params,
            Some(BTreeMap::from([("mount".to_string(), "/".to_string())]))
        );
        assert_eq!(h.items[1].interval.as_secs(), 300);
        assert_eq!(h.triggers[0].severity, Severity::High);
    }

    #[test]
    fn host_file_rejects_unknown_field() {
        let err = serde_json::from_str::<HostFile>(r#"{"host":"a","itmes":[]}"#).unwrap_err();
        assert!(err.to_string().contains("itmes"), "{err}");
    }

    #[test]
    fn template_and_notification_files_parse() {
        let t: TemplateFile = serde_json::from_str(r#"{"template":"linux-base"}"#).unwrap();
        assert!(t.items.is_empty() && t.triggers.is_empty());
        let n: NotificationFile = serde_json::from_str(
            r#"{"name":"ops","email":["a@example.com"],"min_severity":"warning"}"#,
        )
        .unwrap();
        assert_eq!(n.min_severity, Severity::Warning);
        assert!(n.groups.is_empty());
    }
}
