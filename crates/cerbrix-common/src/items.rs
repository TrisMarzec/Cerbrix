//! The built-in item catalogue (see `docs/items.md`).

use crate::agent_config::ConfiguredItem;

/// One built-in item: its key, the agent collector that emits it and its parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemSpec {
    pub key: &'static str,
    /// One of `cpu`, `load`, `memory`, `uptime`, `disk`, `net`.
    pub collector: &'static str,
    /// The parameter that selects an instance (`mount` or `interface`), if any.
    pub param: Option<&'static str>,
}

const fn spec(key: &'static str, collector: &'static str, param: Option<&'static str>) -> ItemSpec {
    ItemSpec {
        key,
        collector,
        param,
    }
}

/// The 17 built-in item keys.
pub const BUILTIN_ITEMS: &[ItemSpec] = &[
    spec("cpu.usage", "cpu", None),
    spec("system.load1", "load", None),
    spec("system.load5", "load", None),
    spec("system.load15", "load", None),
    spec("memory.total_bytes", "memory", None),
    spec("memory.used_bytes", "memory", None),
    spec("memory.available_bytes", "memory", None),
    spec("memory.used_percent", "memory", None),
    spec("swap.total_bytes", "memory", None),
    spec("swap.used_bytes", "memory", None),
    spec("swap.used_percent", "memory", None),
    spec("system.uptime_seconds", "uptime", None),
    spec("disk.total_bytes", "disk", Some("mount")),
    spec("disk.used_bytes", "disk", Some("mount")),
    spec("disk.used_percent", "disk", Some("mount")),
    spec("net.rx_bytes_per_sec", "net", Some("interface")),
    spec("net.tx_bytes_per_sec", "net", Some("interface")),
];

/// Looks up a built-in item by key.
pub fn builtin_item(key: &str) -> Option<&'static ItemSpec> {
    BUILTIN_ITEMS.iter().find(|s| s.key == key)
}

/// The name of the series that stores an item: `disk.used_percent` becomes
/// `cerbrix_disk_used_percent`.
pub fn metric_name(key: &str) -> String {
    format!("cerbrix_{}", key.replace('.', "_"))
}

/// The built-in item stored in the series `name`, if there is one.
pub fn key_for_metric(name: &str) -> Option<&'static str> {
    BUILTIN_ITEMS
        .iter()
        .map(|s| s.key)
        .find(|key| metric_name(key) == name)
}

/// Every built-in item without parameters, at the M1 default intervals
/// (30 s for cpu, load, memory and net; 300 s for uptime and disk).
pub fn default_agent_items() -> Vec<ConfiguredItem> {
    BUILTIN_ITEMS
        .iter()
        .map(|s| ConfiguredItem {
            key: s.key.to_string(),
            params: None,
            interval_secs: match s.collector {
                "uptime" | "disk" => 300,
                _ => 30,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate_item_key;

    #[test]
    fn catalogue_matches_items_md() {
        assert_eq!(BUILTIN_ITEMS.len(), 17);
        assert_eq!(
            builtin_item("disk.used_percent").unwrap().param,
            Some("mount")
        );
        assert!(builtin_item("disk.usedpercent").is_none());
        for s in BUILTIN_ITEMS {
            assert_eq!(validate_item_key(s.key), Ok(()), "{}", s.key);
        }
    }

    #[test]
    fn metric_name_round_trips_for_every_builtin_item() {
        assert_eq!(metric_name("cpu.usage"), "cerbrix_cpu_usage");
        assert_eq!(
            metric_name("disk.used_percent"),
            "cerbrix_disk_used_percent"
        );
        for s in BUILTIN_ITEMS {
            assert_eq!(
                key_for_metric(&metric_name(s.key)),
                Some(s.key),
                "{}",
                s.key
            );
        }
    }

    #[test]
    fn key_for_unknown_metric_is_none() {
        for name in [
            "",
            "cpu_usage",
            "cerbrix_",
            "cerbrix_cpu_usag",
            "cerbrix_cpu_usage_x",
            "up",
            "Cerbrix_cpu_usage",
        ] {
            assert_eq!(key_for_metric(name), None, "{name}");
        }
    }

    #[test]
    fn default_items_use_m1_intervals() {
        let items = default_agent_items();
        assert_eq!(items.len(), 17);
        let secs = |k: &str| items.iter().find(|i| i.key == k).unwrap().interval_secs;
        assert_eq!(secs("cpu.usage"), 30);
        assert_eq!(secs("disk.used_percent"), 300);
        assert_eq!(secs("system.uptime_seconds"), 300);
    }
}
