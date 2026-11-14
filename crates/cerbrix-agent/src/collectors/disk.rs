//! Disk collector: size and usage per mounted file system.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Context;
use sysinfo::Disks;

use super::filter::{Patterns, is_valid_param_value};
use crate::collector::{Collector, Reading};

/// File-system types that are not real storage; they are skipped by default.
pub const DEFAULT_EXCLUDED_FS: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "tmpfs",
    "cgroup",
    "cgroup2",
    "overlay",
    "squashfs",
    "autofs",
    "debugfs",
    "tracefs",
    "securityfs",
    "pstore",
    "bpf",
    "mqueue",
    "hugetlbfs",
    "fusectl",
    "configfs",
    "ramfs",
    "nsfs",
];

/// One mounted file system as reported by a [`DiskSource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskInfo {
    /// Device name (for example `/dev/sda1`); empty if unknown.
    pub device: String,
    pub mount: String,
    pub fs: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Raw disk data; a trait so tests can supply fixed lists.
pub(crate) trait DiskSource: Send {
    fn disks(&mut self) -> anyhow::Result<Vec<DiskInfo>>;
}

/// Reads the mounted file systems through `sysinfo`.
struct SysinfoDisks {
    disks: Option<Disks>,
}

impl DiskSource for SysinfoDisks {
    fn disks(&mut self) -> anyhow::Result<Vec<DiskInfo>> {
        let disks = match self.disks.as_mut() {
            Some(disks) => disks,
            None => self
                .disks
                .insert(Disks::new().context("initialising disk information")?),
        };
        // `true` drops disks that are no longer mounted; the list itself is
        // re-read on every refresh, so new mounts show up too.
        disks.refresh(true);
        Ok(disks
            .iter()
            .map(|d| DiskInfo {
                device: d.name().to_string_lossy().into_owned(),
                mount: d.mount_point().to_string_lossy().into_owned(),
                fs: d.file_system().to_string_lossy().into_owned(),
                total_bytes: d.total_space(),
                available_bytes: d.available_space(),
            })
            .collect())
    }
}

/// Total bytes, used bytes and used percent for every mount. Mounts are
/// discovered on every collection.
pub struct DiskCollector {
    source: Box<dyn DiskSource>,
    /// Lower-case file-system types to skip.
    exclude_fs: Vec<String>,
    exclude_mounts: Patterns,
}

impl DiskCollector {
    /// `exclude_fs` is compared case-insensitively against the file-system
    /// type; `exclude_mounts` is matched against the mount point.
    pub fn new(exclude_fs: Vec<String>, exclude_mounts: Patterns) -> Self {
        Self::with_source(
            Box::new(SysinfoDisks { disks: None }),
            exclude_fs,
            exclude_mounts,
        )
    }

    pub(crate) fn with_source(
        source: Box<dyn DiskSource>,
        exclude_fs: Vec<String>,
        exclude_mounts: Patterns,
    ) -> Self {
        Self {
            source,
            exclude_fs: exclude_fs.iter().map(|s| s.to_lowercase()).collect(),
            exclude_mounts,
        }
    }
}

impl Collector for DiskCollector {
    fn name(&self) -> &'static str {
        "disk"
    }

    fn collect(&mut self) -> anyhow::Result<Vec<Reading>> {
        let disks = self.source.disks()?;
        let mut invalid = Vec::new();
        let mut candidates = Vec::new();
        for disk in disks {
            let fs = disk.fs.to_lowercase();
            if self.exclude_fs.contains(&fs)
                || self.exclude_mounts.matches(&disk.mount)
                || disk.total_bytes == 0
            {
                continue;
            }
            if !is_valid_param_value(&disk.mount) {
                invalid.push(disk.mount);
                continue;
            }
            candidates.push(disk);
        }

        // A device mounted at several points (bind mounts, also those made by the
        // service sandbox) is reported once, at its shortest mount point; ties go to
        // the first listed. Entries without a device name are never merged.
        let mut shortest: HashMap<&str, usize> = HashMap::new();
        for (i, disk) in candidates.iter().enumerate() {
            if disk.device.is_empty() {
                continue;
            }
            let best = shortest.entry(disk.device.as_str()).or_insert(i);
            if disk.mount.len() < candidates[*best].mount.len() {
                *best = i;
            }
        }
        let keep: Vec<bool> = candidates
            .iter()
            .enumerate()
            .map(|(i, disk)| disk.device.is_empty() || shortest[disk.device.as_str()] == i)
            .collect();

        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (disk, keep) in candidates.into_iter().zip(keep) {
            if !keep || !seen.insert(disk.mount.clone()) {
                continue;
            }
            let used = disk.total_bytes.saturating_sub(disk.available_bytes);
            let percent = (used as f64 / disk.total_bytes as f64 * 100.0).clamp(0.0, 100.0);
            let params = BTreeMap::from([("mount".to_owned(), disk.mount)]);
            for (key, value) in [
                ("disk.total_bytes", disk.total_bytes as f64),
                ("disk.used_bytes", used as f64),
                ("disk.used_percent", percent),
            ] {
                out.push(Reading {
                    key,
                    params: params.clone(),
                    value,
                });
            }
        }
        if !invalid.is_empty() {
            tracing::warn!(
                count = invalid.len(),
                mounts = ?invalid,
                "skipping mount points that are not valid parameter values"
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Shared = Arc<Mutex<Vec<DiskInfo>>>;

    struct FakeDisks(Shared);

    impl DiskSource for FakeDisks {
        fn disks(&mut self) -> anyhow::Result<Vec<DiskInfo>> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn info(mount: &str, fs: &str, total: u64, available: u64) -> DiskInfo {
        info_dev(mount, fs, "", total, available)
    }

    fn info_dev(mount: &str, fs: &str, device: &str, total: u64, available: u64) -> DiskInfo {
        DiskInfo {
            device: device.into(),
            mount: mount.into(),
            fs: fs.into(),
            total_bytes: total,
            available_bytes: available,
        }
    }

    fn collector(disks: Vec<DiskInfo>, mount_globs: &[&str]) -> (DiskCollector, Shared) {
        let shared = Arc::new(Mutex::new(disks));
        let patterns = Patterns::new(
            &mount_globs
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let exclude_fs = DEFAULT_EXCLUDED_FS.iter().map(|s| s.to_string()).collect();
        (
            DiskCollector::with_source(Box::new(FakeDisks(shared.clone())), exclude_fs, patterns),
            shared,
        )
    }

    fn mounts(readings: &[Reading]) -> Vec<&str> {
        let mut m: Vec<&str> = readings
            .iter()
            .map(|r| r.params["mount"].as_str())
            .collect();
        m.dedup();
        m
    }

    #[test]
    fn reports_three_keys_per_mount() {
        let (mut c, _) = collector(
            vec![info("/", "ext4", 100, 60), info("/data", "xfs", 1000, 1000)],
            &[],
        );
        let r = c.collect().unwrap();
        assert_eq!(r.len(), 6);
        let root = |key: &str| {
            r.iter()
                .find(|x| x.key == key && x.params["mount"] == "/")
                .unwrap()
                .value
        };
        assert_eq!(root("disk.total_bytes"), 100.0);
        assert_eq!(root("disk.used_bytes"), 40.0);
        assert_eq!(root("disk.used_percent"), 40.0);
        assert_eq!(mounts(&r), ["/", "/data"]);
    }

    #[test]
    fn excludes_default_pseudo_filesystems() {
        let (mut c, _) = collector(
            vec![
                info("/run", "tmpfs", 100, 50),
                info("/dev", "TmpFS", 100, 50),
            ],
            &[],
        );
        assert!(c.collect().unwrap().is_empty());
    }

    #[test]
    fn excludes_mounts_by_pattern() {
        let (mut c, _) = collector(
            vec![
                info("/var/lib/docker/overlay2/x", "ext4", 100, 50),
                info("/", "ext4", 100, 50),
            ],
            &["/var/lib/docker/*"],
        );
        assert_eq!(mounts(&c.collect().unwrap()), ["/"]);
    }

    #[test]
    fn duplicate_mount_points_are_reported_once() {
        let (mut c, _) = collector(
            vec![info("/", "ext4", 100, 50), info("/", "ext4", 100, 50)],
            &[],
        );
        assert_eq!(c.collect().unwrap().len(), 3);
    }

    #[test]
    fn bind_mounts_of_one_device_are_reported_once() {
        let (mut c, _) = collector(
            vec![
                info_dev("/var/lib/cerbrix-agent", "ext4", "/dev/sda1", 100, 50),
                info_dev("/", "ext4", "/dev/sda1", 100, 50),
                info_dev("/home", "ext4", "/dev/sda1", 100, 50),
            ],
            &[],
        );
        assert_eq!(mounts(&c.collect().unwrap()), ["/"]);
    }

    #[test]
    fn different_devices_are_all_reported() {
        let (mut c, _) = collector(
            vec![
                info_dev("/", "ext4", "/dev/sda1", 100, 50),
                info_dev("/home", "ext4", "/dev/sdb1", 100, 50),
            ],
            &[],
        );
        assert_eq!(mounts(&c.collect().unwrap()), ["/", "/home"]);
    }

    #[test]
    fn empty_device_names_are_not_merged() {
        let (mut c, _) = collector(
            vec![info("/", "ext4", 100, 50), info("/data", "ext4", 100, 50)],
            &[],
        );
        assert_eq!(mounts(&c.collect().unwrap()), ["/", "/data"]);
    }

    #[test]
    fn excluded_mount_does_not_shadow_a_longer_one() {
        let (mut c, _) = collector(
            vec![
                info_dev("/", "ext4", "/dev/sda1", 100, 50),
                info_dev("/srv/data", "ext4", "/dev/sda1", 100, 50),
                info_dev("/srv/data/deep", "ext4", "/dev/sda1", 100, 50),
            ],
            &["/"],
        );
        assert_eq!(mounts(&c.collect().unwrap()), ["/srv/data"]);
    }

    #[test]
    fn zero_sized_disks_are_skipped() {
        let (mut c, _) = collector(vec![info("/mnt/empty", "ext4", 0, 0)], &[]);
        assert!(c.collect().unwrap().is_empty());
    }

    #[test]
    fn new_mount_appears_without_restart() {
        let (mut c, shared) = collector(vec![info("/", "ext4", 100, 50)], &[]);
        assert_eq!(mounts(&c.collect().unwrap()), ["/"]);
        shared.lock().unwrap().push(info("/data", "ext4", 100, 50));
        assert_eq!(mounts(&c.collect().unwrap()), ["/", "/data"]);
        shared.lock().unwrap().remove(0);
        assert_eq!(mounts(&c.collect().unwrap()), ["/data"]);
    }

    #[test]
    fn invalid_mount_names_are_skipped() {
        let (mut c, _) = collector(
            vec![
                info("/bad\nname", "ext4", 100, 50),
                info("/ok", "ext4", 100, 50),
            ],
            &[],
        );
        assert_eq!(mounts(&c.collect().unwrap()), ["/ok"]);
    }

    #[test]
    fn available_above_total_does_not_underflow() {
        let (mut c, _) = collector(vec![info("/", "ext4", 100, 150)], &[]);
        let r = c.collect().unwrap();
        assert_eq!(r[1].value, 0.0);
    }

    #[test]
    fn real_disks_produce_valid_readings() {
        let mut c = DiskCollector::new(
            DEFAULT_EXCLUDED_FS.iter().map(|s| s.to_string()).collect(),
            Patterns::default(),
        );
        for r in c.collect().unwrap() {
            cerbrix_common::validate_item_key(r.key).unwrap();
            assert!(r.value >= 0.0);
            assert!(is_valid_param_value(&r.params["mount"]));
        }
    }
}
