//! The configuration the agent gets from the server: a cache of it on disk and
//! the plan that turns its items into groups of collectors.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use cerbrix_common::agent_config::{ConfiguredItem, ItemConfig};
use cerbrix_common::items::{ItemSpec, builtin_item};

/// Largest cache file `ConfigCache::load` accepts.
pub(crate) const MAX_CACHE_BYTES: u64 = 1024 * 1024;

/// The last configuration received, kept in `<state_dir>/config.json`.
#[derive(Debug, Clone)]
pub struct ConfigCache {
    path: PathBuf,
}

impl ConfigCache {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            path: state_dir.join("config.json"),
        }
    }

    /// The cached configuration. A missing, oversized, unparseable or
    /// versionless file yields `None` (with a warning, except when missing).
    pub fn load(&self) -> Option<ItemConfig> {
        let file = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!("cannot read the cached configuration: {e}");
                return None;
            }
        };
        let mut bytes = Vec::new();
        if let Err(e) = file.take(MAX_CACHE_BYTES + 1).read_to_end(&mut bytes) {
            tracing::warn!("cannot read the cached configuration: {e}");
            return None;
        }
        if bytes.len() as u64 > MAX_CACHE_BYTES {
            tracing::warn!("ignoring the cached configuration: file is too large");
            return None;
        }
        match serde_json::from_slice::<ItemConfig>(&bytes) {
            Ok(config) if !config.version.is_empty() => Some(config),
            Ok(_) => {
                tracing::warn!("ignoring the cached configuration: empty version");
                None
            }
            Err(e) => {
                tracing::warn!("ignoring the cached configuration: {e}");
                None
            }
        }
    }

    /// Stores `config` atomically: a temp file in the same directory, then a rename.
    pub fn save(&self, config: &ItemConfig) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("tmp");
        match std::fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let json = serde_json::to_vec(config).map_err(io::Error::other)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options.open(&tmp)?;
            file.write_all(&json)?;
            file.sync_all()?;
            std::fs::rename(&tmp, &self.path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

/// The built-in collectors, in the order `plan` sorts groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CollectorKind {
    Cpu,
    Load,
    Memory,
    Uptime,
    Disk,
    Net,
}

impl CollectorKind {
    /// The collector that emits `spec`'s item; `None` for an unknown collector name.
    pub fn from_spec(spec: &ItemSpec) -> Option<Self> {
        Some(match spec.collector {
            "cpu" => Self::Cpu,
            "load" => Self::Load,
            "memory" => Self::Memory,
            "uptime" => Self::Uptime,
            "disk" => Self::Disk,
            "net" => Self::Net,
            _ => return None,
        })
    }
}

/// One item a group's collector should report.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Wanted {
    pub key: String,
    /// `Some`: exactly this instance. `None`: every instance not in `except`.
    pub params: Option<BTreeMap<String, String>>,
    pub except: Vec<BTreeMap<String, String>>,
}

/// Items of one collector that share an interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub collector: CollectorKind,
    pub interval: Duration,
    pub wanted: Vec<Wanted>,
}

/// Turns the configured items into groups. Unknown keys, collectors, parameter
/// names and zero intervals are skipped with a warning; of duplicate
/// `(key, params)` entries the first wins. The result is deterministic.
pub fn plan(items: &[ConfiguredItem]) -> Vec<Group> {
    struct Accepted {
        kind: CollectorKind,
        key: String,
        params: Option<BTreeMap<String, String>>,
        interval: Duration,
    }

    let mut accepted: Vec<Accepted> = Vec::new();
    for item in items {
        let Some(spec) = builtin_item(&item.key) else {
            tracing::warn!(key = %item.key, "skipping an item the agent does not know");
            continue;
        };
        let Some(kind) = CollectorKind::from_spec(spec) else {
            tracing::warn!(key = %item.key, "skipping an item with an unknown collector");
            continue;
        };
        if item.interval_secs == 0 {
            tracing::warn!(key = %item.key, "skipping an item with an interval of 0");
            continue;
        }
        let params = item.params.clone().filter(|p| !p.is_empty());
        if let Some(p) = &params {
            let fits = spec
                .param
                .is_some_and(|name| p.len() == 1 && p.contains_key(name));
            if !fits {
                tracing::warn!(key = %item.key, "skipping an item with unsuitable params");
                continue;
            }
        }
        if accepted
            .iter()
            .any(|a| a.key == item.key && a.params == params)
        {
            continue;
        }
        accepted.push(Accepted {
            kind,
            key: item.key.clone(),
            params,
            interval: Duration::from_secs(item.interval_secs),
        });
    }

    let mut groups: BTreeMap<(CollectorKind, Duration), Vec<Wanted>> = BTreeMap::new();
    for a in &accepted {
        let except = if a.params.is_none() {
            let mut specific: Vec<_> = accepted
                .iter()
                .filter(|o| o.key == a.key)
                .filter_map(|o| o.params.clone())
                .collect();
            specific.sort();
            specific
        } else {
            Vec::new()
        };
        groups
            .entry((a.kind, a.interval))
            .or_default()
            .push(Wanted {
                key: a.key.clone(),
                params: a.params.clone(),
                except,
            });
    }
    groups
        .into_iter()
        .map(|((collector, interval), mut wanted)| {
            wanted.sort();
            Group {
                collector,
                interval,
                wanted,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, params: Option<&[(&str, &str)]>, secs: u64) -> ConfiguredItem {
        ConfiguredItem {
            key: key.to_owned(),
            params: params.map(|p| {
                p.iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect()
            }),
            interval_secs: secs,
        }
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn plan_groups_by_collector_and_interval() {
        let groups = plan(&[
            item("memory.total_bytes", None, 3600),
            item("cpu.usage", None, 30),
            item("memory.used_percent", None, 30),
        ]);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].collector, CollectorKind::Cpu);
        assert_eq!(groups[0].interval, Duration::from_secs(30));
        assert_eq!(groups[1].collector, CollectorKind::Memory);
        assert_eq!(groups[1].interval, Duration::from_secs(30));
        assert_eq!(groups[1].wanted.len(), 1);
        assert_eq!(groups[1].wanted[0].key, "memory.used_percent");
        assert_eq!(groups[2].collector, CollectorKind::Memory);
        assert_eq!(groups[2].interval, Duration::from_secs(3600));
        assert_eq!(groups[2].wanted.len(), 1);
        assert_eq!(groups[2].wanted[0].key, "memory.total_bytes");
    }

    #[test]
    fn plan_specific_instance_excluded_from_general() {
        let groups = plan(&[
            item("disk.used_percent", None, 300),
            item("disk.used_percent", Some(&[("mount", "/")]), 60),
        ]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].collector, CollectorKind::Disk);
        assert_eq!(groups[0].interval, Duration::from_secs(60));
        assert_eq!(
            groups[0].wanted,
            vec![Wanted {
                key: "disk.used_percent".into(),
                params: Some(map(&[("mount", "/")])),
                except: vec![],
            }]
        );
        assert_eq!(groups[1].interval, Duration::from_secs(300));
        assert_eq!(
            groups[1].wanted,
            vec![Wanted {
                key: "disk.used_percent".into(),
                params: None,
                except: vec![map(&[("mount", "/")])],
            }]
        );
    }

    #[test]
    fn plan_skips_unknown_key_and_zero_interval() {
        let groups = plan(&[
            item("gpu.usage", None, 30),
            item("cpu.usage", None, 0),
            item("cpu.usage", None, 30),
        ]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].collector, CollectorKind::Cpu);
        assert_eq!(groups[0].interval, Duration::from_secs(30));
    }

    #[test]
    fn plan_skips_wrong_param_name_and_treats_empty_params_as_none() {
        let groups = plan(&[
            item("cpu.usage", Some(&[("mount", "/")]), 30),
            item("disk.used_percent", Some(&[("interface", "eth0")]), 30),
            item("disk.used_percent", Some(&[]), 60),
        ]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].interval, Duration::from_secs(60));
        assert_eq!(groups[0].wanted[0].params, None);
        assert!(groups[0].wanted[0].except.is_empty());
    }

    #[test]
    fn plan_first_duplicate_wins() {
        let groups = plan(&[item("cpu.usage", None, 30), item("cpu.usage", None, 60)]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].interval, Duration::from_secs(30));
    }

    #[test]
    fn plan_is_deterministic() {
        let a = [
            item("net.rx_bytes_per_sec", None, 30),
            item("disk.used_bytes", None, 300),
            item("disk.total_bytes", None, 300),
            item("cpu.usage", None, 30),
            item("disk.total_bytes", Some(&[("mount", "/b")]), 300),
            item("disk.total_bytes", Some(&[("mount", "/a")]), 300),
        ];
        let mut b = a.clone();
        b.reverse();
        let (pa, pb) = (plan(&a), plan(&b));
        // Same shape regardless of input order (the winner of duplicates aside,
        // there are none here).
        assert_eq!(pa.len(), pb.len());
        for (x, y) in pa.iter().zip(&pb) {
            assert_eq!(x.collector, y.collector);
            assert_eq!(x.interval, y.interval);
            assert_eq!(x.wanted, y.wanted);
        }
        let order: Vec<_> = pa.iter().map(|g| g.collector).collect();
        assert_eq!(
            order,
            [CollectorKind::Cpu, CollectorKind::Disk, CollectorKind::Net]
        );
        let keys: Vec<_> = pa[1]
            .wanted
            .iter()
            .map(|w| (w.key.as_str(), w.params.clone()))
            .collect();
        assert_eq!(keys[0].0, "disk.total_bytes");
        assert_eq!(keys[0].1, None);
        assert_eq!(keys[1].1, Some(map(&[("mount", "/a")])));
        assert_eq!(keys[2].1, Some(map(&[("mount", "/b")])));
        assert_eq!(keys[3].0, "disk.used_bytes");
    }

    #[test]
    fn plan_of_nothing_is_empty() {
        assert!(plan(&[]).is_empty());
    }

    #[test]
    fn collector_kind_from_spec() {
        let spec = cerbrix_common::items::builtin_item("net.rx_bytes_per_sec").unwrap();
        assert_eq!(CollectorKind::from_spec(spec), Some(CollectorKind::Net));
        let odd = ItemSpec {
            key: "x.y",
            collector: "gpu",
            param: None,
        };
        assert_eq!(CollectorKind::from_spec(&odd), None);
    }

    fn config(version: &str) -> ItemConfig {
        ItemConfig {
            version: version.to_owned(),
            items: vec![item("cpu.usage", None, 30)],
        }
    }

    #[test]
    fn cache_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ConfigCache::new(dir.path());
        assert_eq!(cache.load(), None);
        cache.save(&config("v1")).unwrap();
        assert_eq!(cache.load(), Some(config("v1")));
        cache.save(&config("v2")).unwrap();
        assert_eq!(cache.load(), Some(config("v2")));
    }

    #[test]
    fn cache_ignores_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ConfigCache::new(dir.path());
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{").unwrap();
        assert_eq!(cache.load(), None);
        std::fs::write(&path, r#"{"version":"","items":[]}"#).unwrap();
        assert_eq!(cache.load(), None);
        std::fs::write(&path, vec![b' '; MAX_CACHE_BYTES as usize + 1]).unwrap();
        assert_eq!(cache.load(), None);
    }

    #[test]
    fn cache_save_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ConfigCache::new(dir.path());
        cache.save(&config("v1")).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["config.json"]);

        // A directory squatting on the temp file name makes the save fail
        // before the old cache is touched.
        std::fs::create_dir(dir.path().join("config.tmp")).unwrap();
        assert!(cache.save(&config("v2")).is_err());
        assert_eq!(cache.load(), Some(config("v1")));

        // The state directory vanishing is an error, not a panic.
        let gone = ConfigCache::new(&dir.path().join("a").join("b"));
        std::fs::write(dir.path().join("a"), "a file, not a directory").unwrap();
        assert!(gone.save(&config("v3")).is_err());
    }
}
