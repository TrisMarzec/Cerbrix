//! System collectors: CPU, load average, memory, swap and uptime.

use std::collections::BTreeMap;

use anyhow::Context;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, System};

use crate::collector::{Collector, Reading};

fn reading(key: &'static str, value: f64) -> Reading {
    Reading {
        key,
        params: BTreeMap::new(),
        value,
    }
}

/// `used / total` as a percentage; 0 when `total` is 0.
fn percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
    }
}

/// Average CPU usage over all cores, measured between two consecutive
/// `collect` calls. The first call only primes the counters and returns no
/// reading.
pub struct CpuCollector {
    system: Option<System>,
}

impl CpuCollector {
    pub fn new() -> Self {
        Self { system: None }
    }
}

impl Default for CpuCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector for CpuCollector {
    fn name(&self) -> &'static str {
        "cpu"
    }

    fn collect(&mut self) -> anyhow::Result<Vec<Reading>> {
        let Some(system) = self.system.as_mut() else {
            let mut system = System::new().context("initialising system information")?;
            system.refresh_cpu_specifics(CpuRefreshKind::nothing().with_usage());
            self.system = Some(system);
            return Ok(Vec::new());
        };
        system.refresh_cpu_specifics(CpuRefreshKind::nothing().with_usage());
        let usage = f64::from(system.global_cpu_usage()).clamp(0.0, 100.0);
        Ok(vec![reading("cpu.usage", usage)])
    }
}

/// 1, 5 and 15 minute load averages (always zero on Windows).
pub struct LoadCollector;

impl LoadCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LoadCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector for LoadCollector {
    fn name(&self) -> &'static str {
        "load"
    }

    fn collect(&mut self) -> anyhow::Result<Vec<Reading>> {
        let (one, five, fifteen) = load_average()?;
        Ok(vec![
            reading("system.load1", one),
            reading("system.load5", five),
            reading("system.load15", fifteen),
        ])
    }
}

#[cfg(not(windows))]
fn load_average() -> anyhow::Result<(f64, f64, f64)> {
    let load = System::load_average().context("reading load average")?;
    Ok((load.one, load.five, load.fifteen))
}

/// Windows has no load average and `sysinfo` reports it as unsupported, so the
/// items are reported as zero to keep the item set the same on every platform.
#[cfg(windows)]
fn load_average() -> anyhow::Result<(f64, f64, f64)> {
    Ok((0.0, 0.0, 0.0))
}

/// RAM and swap, in bytes and percent.
pub struct MemoryCollector {
    system: Option<System>,
}

impl MemoryCollector {
    pub fn new() -> Self {
        Self { system: None }
    }
}

impl Default for MemoryCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector for MemoryCollector {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn collect(&mut self) -> anyhow::Result<Vec<Reading>> {
        if self.system.is_none() {
            self.system = Some(System::new().context("initialising system information")?);
        }
        let system = self.system.as_mut().expect("initialised above");
        system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap());

        let total = system.total_memory();
        let used = system.used_memory();
        let swap_total = system.total_swap();
        let swap_used = system.used_swap();

        let mut out = vec![
            reading("memory.total_bytes", total as f64),
            reading("memory.used_bytes", used as f64),
            reading("memory.available_bytes", system.available_memory() as f64),
            reading("memory.used_percent", percent(used, total)),
            reading("swap.total_bytes", swap_total as f64),
            reading("swap.used_bytes", swap_used as f64),
        ];
        if swap_total > 0 {
            out.push(reading("swap.used_percent", percent(swap_used, swap_total)));
        }
        Ok(out)
    }
}

/// Seconds since boot.
pub struct UptimeCollector;

impl UptimeCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UptimeCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector for UptimeCollector {
    fn name(&self) -> &'static str {
        "uptime"
    }

    fn collect(&mut self) -> anyhow::Result<Vec<Reading>> {
        let secs = System::uptime().context("reading uptime")?;
        Ok(vec![reading("system.uptime_seconds", secs as f64)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::{Collector, Reading};
    use std::time::Duration;

    fn value(readings: &[Reading], key: &str) -> f64 {
        readings
            .iter()
            .find(|r| r.key == key)
            .unwrap_or_else(|| panic!("missing {key}"))
            .value
    }

    #[test]
    fn cpu_first_call_is_empty_then_reports_percent() {
        let mut c = CpuCollector::new();
        assert!(c.collect().unwrap().is_empty());
        std::thread::sleep(Duration::from_millis(250));
        let r = c.collect().unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "cpu.usage");
        assert!((0.0..=100.0).contains(&r[0].value), "{}", r[0].value);
    }

    #[test]
    fn memory_readings_are_consistent() {
        let r = MemoryCollector::new().collect().unwrap();
        let total = value(&r, "memory.total_bytes");
        let used = value(&r, "memory.used_bytes");
        let avail = value(&r, "memory.available_bytes");
        assert!(total > 0.0);
        assert!(used + avail <= total * 1.05);
        assert!((0.0..=100.0).contains(&value(&r, "memory.used_percent")));
        let _ = value(&r, "swap.total_bytes");
        let _ = value(&r, "swap.used_bytes");
        if value(&r, "swap.total_bytes") == 0.0 {
            assert!(r.iter().all(|x| x.key != "swap.used_percent"));
        } else {
            assert!((0.0..=100.0).contains(&value(&r, "swap.used_percent")));
        }
    }

    #[test]
    fn uptime_is_positive() {
        let r = UptimeCollector::new().collect().unwrap();
        assert!(value(&r, "system.uptime_seconds") > 0.0);
    }

    #[test]
    fn all_keys_are_valid_item_keys() {
        let mut cpu = CpuCollector::new();
        cpu.collect().unwrap();
        std::thread::sleep(Duration::from_millis(250));
        let mut all = cpu.collect().unwrap();
        all.extend(LoadCollector::new().collect().unwrap());
        all.extend(MemoryCollector::new().collect().unwrap());
        all.extend(UptimeCollector::new().collect().unwrap());
        assert!(!all.is_empty());
        for r in &all {
            cerbrix_common::validate_item_key(r.key).unwrap_or_else(|e| panic!("{}: {e}", r.key));
            assert!(r.params.is_empty());
        }
    }

    #[test]
    fn load_returns_three_values() {
        let r = LoadCollector::new().collect().unwrap();
        assert_eq!(r.len(), 3);
        for key in ["system.load1", "system.load5", "system.load15"] {
            assert!(value(&r, key) >= 0.0);
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn load_reports_three_values() {
        let r = LoadCollector::new().collect().unwrap();
        assert_eq!(r.len(), 3);
    }
}
