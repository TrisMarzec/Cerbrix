//! Network collector: receive and transmit rate per interface.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use anyhow::Context;
use sysinfo::Networks;

use super::filter::{Patterns, is_valid_param_value};
use crate::collector::{Collector, Reading};

/// One interface with its cumulative byte counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetInfo {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Raw interface data; a trait so tests can supply fixed lists.
pub(crate) trait NetSource: Send {
    fn interfaces(&mut self) -> anyhow::Result<Vec<NetInfo>>;
}

/// Reads the interfaces through `sysinfo`.
struct SysinfoNetworks {
    networks: Option<Networks>,
}

impl NetSource for SysinfoNetworks {
    fn interfaces(&mut self) -> anyhow::Result<Vec<NetInfo>> {
        let networks = match self.networks.as_mut() {
            Some(networks) => networks,
            None => self
                .networks
                .insert(Networks::new().context("initialising network information")?),
        };
        // `true` drops interfaces that disappeared; new ones are added.
        networks.refresh(true);
        Ok(networks
            .iter()
            .map(|(name, data)| NetInfo {
                name: name.to_string_lossy().into_owned(),
                rx_bytes: data.total_received(),
                tx_bytes: data.total_transmitted(),
            })
            .collect())
    }
}

/// Time and `(rx, tx)` counters of one collection.
type Snapshot = (Instant, HashMap<String, (u64, u64)>);

/// Loopback: `lo`, `lo` followed only by digits (`lo0`), or a Windows
/// "Loopback" pseudo-interface.
fn is_loopback(name: &str) -> bool {
    name.strip_prefix("lo")
        .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit()))
        || name.contains("Loopback")
}

/// Receive and transmit rate in bytes per second for every interface, measured
/// between two consecutive `collect` calls. The first call only records the
/// counters and returns no reading. Interfaces are discovered on every
/// collection; loopback is always excluded.
pub struct NetCollector {
    source: Box<dyn NetSource>,
    exclude: Patterns,
    clock: Box<dyn Fn() -> Instant + Send>,
    /// Counters and time of the previous collection.
    previous: Option<Snapshot>,
}

impl NetCollector {
    pub fn new(exclude_interfaces: Patterns) -> Self {
        Self::with_source(
            Box::new(SysinfoNetworks { networks: None }),
            exclude_interfaces,
            Box::new(Instant::now),
        )
    }

    pub(crate) fn with_source(
        source: Box<dyn NetSource>,
        exclude: Patterns,
        clock: Box<dyn Fn() -> Instant + Send>,
    ) -> Self {
        Self {
            source,
            exclude,
            clock,
            previous: None,
        }
    }
}

impl Collector for NetCollector {
    fn name(&self) -> &'static str {
        "net"
    }

    fn collect(&mut self) -> anyhow::Result<Vec<Reading>> {
        let interfaces = self.source.interfaces()?;
        let now = (self.clock)();

        let mut current: HashMap<String, (u64, u64)> = HashMap::new();
        let mut invalid = Vec::new();
        for iface in interfaces {
            if is_loopback(&iface.name) || self.exclude.matches(&iface.name) {
                continue;
            }
            if !is_valid_param_value(&iface.name) {
                invalid.push(iface.name);
                continue;
            }
            current
                .entry(iface.name)
                .or_insert((iface.rx_bytes, iface.tx_bytes));
        }
        if !invalid.is_empty() {
            tracing::warn!(
                count = invalid.len(),
                interfaces = ?invalid,
                "skipping interfaces that are not valid parameter values"
            );
        }

        let mut out = Vec::new();
        if let Some((then, before)) = &self.previous {
            let secs = now.saturating_duration_since(*then).as_secs_f64();
            if secs > 0.0 {
                let mut names: Vec<&String> = current.keys().collect();
                names.sort();
                for name in names {
                    let (rx, tx) = current[name];
                    let Some(&(prev_rx, prev_tx)) = before.get(name) else {
                        continue;
                    };
                    // A counter that went backwards (reset, wrap, interface
                    // re-created) has no meaningful rate for this round.
                    if rx < prev_rx || tx < prev_tx {
                        continue;
                    }
                    let params = BTreeMap::from([("interface".to_owned(), name.clone())]);
                    out.push(Reading {
                        key: "net.rx_bytes_per_sec",
                        params: params.clone(),
                        value: (rx - prev_rx) as f64 / secs,
                    });
                    out.push(Reading {
                        key: "net.tx_bytes_per_sec",
                        params,
                        value: (tx - prev_tx) as f64 / secs,
                    });
                }
            }
        }
        self.previous = Some((now, current));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type Shared = Arc<Mutex<Vec<NetInfo>>>;

    struct FakeNet(Shared);

    impl NetSource for FakeNet {
        fn interfaces(&mut self) -> anyhow::Result<Vec<NetInfo>> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn net(name: &str, rx: u64, tx: u64) -> NetInfo {
        NetInfo {
            name: name.into(),
            rx_bytes: rx,
            tx_bytes: tx,
        }
    }

    struct Fixture {
        collector: NetCollector,
        source: Shared,
        now: Arc<Mutex<Instant>>,
    }

    impl Fixture {
        fn new(ifaces: Vec<NetInfo>, globs: &[&str]) -> Self {
            let source = Arc::new(Mutex::new(ifaces));
            let now = Arc::new(Mutex::new(Instant::now()));
            let clock_now = now.clone();
            let patterns =
                Patterns::new(&globs.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
            Self {
                collector: NetCollector::with_source(
                    Box::new(FakeNet(source.clone())),
                    patterns,
                    Box::new(move || *clock_now.lock().unwrap()),
                ),
                source,
                now,
            }
        }

        fn advance(&self, secs: u64) {
            *self.now.lock().unwrap() += Duration::from_secs(secs);
        }
    }

    #[test]
    fn net_first_call_is_empty_then_reports_rates() {
        let mut f = Fixture::new(vec![net("eth0", 5000, 100)], &[]);
        assert!(f.collector.collect().unwrap().is_empty());
        *f.source.lock().unwrap() = vec![net("eth0", 6000, 1100)];
        f.advance(2);
        let r = f.collector.collect().unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].key, "net.rx_bytes_per_sec");
        assert_eq!(r[0].params["interface"], "eth0");
        assert_eq!(r[0].value, 500.0);
        assert_eq!(r[1].key, "net.tx_bytes_per_sec");
        assert_eq!(r[1].value, 500.0);
    }

    #[test]
    fn net_counter_reset_yields_no_reading() {
        let mut f = Fixture::new(vec![net("eth0", 5000, 5000), net("eth1", 0, 0)], &[]);
        f.collector.collect().unwrap();
        *f.source.lock().unwrap() = vec![net("eth0", 10, 6000), net("eth1", 100, 100)];
        f.advance(1);
        let r = f.collector.collect().unwrap();
        assert!(r.iter().all(|x| x.params["interface"] == "eth1"));
        assert_eq!(r.len(), 2);
        // The reset counter becomes the new baseline.
        *f.source.lock().unwrap() = vec![net("eth0", 110, 6100), net("eth1", 200, 200)];
        f.advance(1);
        assert_eq!(f.collector.collect().unwrap().len(), 4);
    }

    #[test]
    fn loopback_is_always_excluded_and_patterns_apply() {
        let mut f = Fixture::new(
            vec![
                net("lo", 0, 0),
                net("lo0", 0, 0),
                net("Loopback Pseudo-Interface 1", 0, 0),
                net("veth12ab", 0, 0),
                net("eth0", 0, 0),
                net("lofty", 0, 0),
            ],
            &["veth*"],
        );
        f.collector.collect().unwrap();
        f.advance(1);
        let r = f.collector.collect().unwrap();
        let mut names: Vec<&str> = r.iter().map(|x| x.params["interface"].as_str()).collect();
        names.dedup();
        assert_eq!(names, ["eth0", "lofty"]);
    }

    #[test]
    fn new_interface_appears_on_the_following_collection() {
        let mut f = Fixture::new(vec![net("eth0", 0, 0)], &[]);
        f.collector.collect().unwrap();
        f.source.lock().unwrap().push(net("eth1", 0, 0));
        f.advance(1);
        assert_eq!(f.collector.collect().unwrap().len(), 2);
        f.advance(1);
        assert_eq!(f.collector.collect().unwrap().len(), 4);
    }

    #[test]
    fn invalid_interface_names_are_skipped() {
        let mut f = Fixture::new(vec![net("bad\nname", 0, 0), net("eth0", 0, 0)], &[]);
        f.collector.collect().unwrap();
        f.advance(1);
        assert_eq!(f.collector.collect().unwrap().len(), 2);
    }

    #[test]
    fn real_interfaces_produce_valid_readings() {
        let mut c = NetCollector::new(Patterns::default());
        assert!(c.collect().unwrap().is_empty());
        std::thread::sleep(Duration::from_millis(50));
        for r in c.collect().unwrap() {
            cerbrix_common::validate_item_key(r.key).unwrap();
            assert!(r.value >= 0.0);
            assert!(is_valid_param_value(&r.params["interface"]));
        }
    }
}
