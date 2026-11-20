//! Counts failed logins per client address, in memory.
//!
//! [`MAX_FAILURES`] failures from one address within [`WINDOW`] refuse further logins from it
//! until the window ends. A window starts with the first failure of an address and ends
//! `WINDOW` later; then the count starts again. An IPv6 address counts as its /64, since one
//! client usually holds a whole /64. The table is forgotten on restart and holds at most
//! [`MAX_ENTRIES`] addresses.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, HeaderName};

/// Failed logins from one address within [`WINDOW`] after which its logins are refused.
pub const MAX_FAILURES: u32 = 10;
/// How long a window lasts, from the first failure of an address.
pub const WINDOW: Duration = Duration::from_secs(300);
/// The most addresses the table holds.
const MAX_ENTRIES: usize = 10_000;

/// What failures are counted under: an IPv4 address, or the /64 of an IPv6 address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum IpKey {
    V4(Ipv4Addr),
    V6(u64),
}

impl From<IpAddr> for IpKey {
    fn from(addr: IpAddr) -> Self {
        // An IPv4 address seen through a dual-stack socket counts as that IPv4 address.
        match addr.to_canonical() {
            IpAddr::V4(v4) => IpKey::V4(v4),
            IpAddr::V6(v6) => IpKey::V6((v6.to_bits() >> 64) as u64),
        }
    }
}

#[derive(Debug)]
struct Entry {
    /// The first failure of the window.
    started: Instant,
    failures: u32,
    /// Whether the refusal of this window was logged.
    warned: bool,
}

impl Entry {
    fn ended(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= WINDOW
    }
}

/// The failed logins of the recent past, per client address. One for the whole process.
#[derive(Debug, Default)]
pub struct LoginThrottle {
    entries: Mutex<HashMap<IpKey, Entry>>,
}

impl LoginThrottle {
    /// True when logins from `addr` are refused at `now`: it had [`MAX_FAILURES`] failures in
    /// a window that has not ended. The first refusal of a window is logged.
    pub fn refused(&self, addr: IpAddr, now: Instant) -> bool {
        // The lock is never held across an await: this function is synchronous.
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(entry) = entries.get_mut(&IpKey::from(addr)) else {
            return false;
        };
        if entry.ended(now) || entry.failures < MAX_FAILURES {
            return false;
        }
        if !entry.warned {
            entry.warned = true;
            tracing::warn!(
                address = %addr,
                "logins from this address are refused for up to 5 minutes after repeated failures"
            );
        }
        true
    }

    /// Counts a failed login from `addr` at `now`. When the table is full, the addresses whose
    /// window has ended make room; if none has, the failure is not recorded, rather than
    /// forgetting the addresses that are refused.
    pub fn failed(&self, addr: IpAddr, now: Instant) {
        let key = IpKey::from(addr);
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(&key)
            && !entry.ended(now)
        {
            entry.failures = entry.failures.saturating_add(1);
            return;
        }
        if !entries.contains_key(&key) && entries.len() >= MAX_ENTRIES {
            entries.retain(|_, entry| !entry.ended(now));
            if entries.len() >= MAX_ENTRIES {
                return;
            }
        }
        entries.insert(
            key,
            Entry {
                started: now,
                failures: 1,
                warned: false,
            },
        );
    }
}

/// The address a request comes from: the right-most address in the last `header` line when
/// the setting names a header, otherwise (and when that header is missing or holds no
/// address) the peer address of the connection.
pub fn client_address(peer: IpAddr, headers: &HeaderMap, header: Option<&HeaderName>) -> IpAddr {
    let Some(header) = header else {
        return peer;
    };
    headers
        .get_all(header)
        .iter()
        .next_back()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit(',').next())
        .and_then(|last| last.trim().parse().ok())
        .unwrap_or(peer)
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn v4(n: u32) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from(n))
    }

    fn fail(throttle: &LoginThrottle, addr: IpAddr, times: u32, now: Instant) {
        for _ in 0..times {
            throttle.failed(addr, now);
        }
    }

    fn len(throttle: &LoginThrottle) -> usize {
        throttle.entries.lock().unwrap().len()
    }

    #[test]
    fn ten_failures_refuse_the_address_until_the_window_ends() {
        let throttle = LoginThrottle::default();
        let addr: IpAddr = "192.0.2.1".parse().unwrap();
        let first = Instant::now();
        assert!(!throttle.refused(addr, first));
        fail(&throttle, addr, 9, first);
        let later = first + Duration::from_secs(10);
        assert!(!throttle.refused(addr, later), "nine failures");
        throttle.failed(addr, later);
        assert!(throttle.refused(addr, later), "the tenth failure");
        assert!(throttle.refused(addr, first + WINDOW - Duration::from_millis(1)));
        // The window ends with its first failure plus WINDOW, not with the last one.
        let ended = first + WINDOW;
        assert!(!throttle.refused(addr, ended));
        // And the count starts again.
        fail(&throttle, addr, 9, ended);
        assert!(!throttle.refused(addr, ended));
        throttle.failed(addr, ended);
        assert!(throttle.refused(addr, ended));
    }

    #[test]
    fn another_address_is_not_affected() {
        let throttle = LoginThrottle::default();
        let now = Instant::now();
        let a: IpAddr = "192.0.2.1".parse().unwrap();
        let b: IpAddr = "192.0.2.2".parse().unwrap();
        fail(&throttle, a, MAX_FAILURES, now);
        assert!(throttle.refused(a, now));
        assert!(!throttle.refused(b, now));
        fail(&throttle, b, MAX_FAILURES - 1, now);
        assert!(!throttle.refused(b, now));
    }

    #[test]
    fn ipv6_counts_per_64() {
        let throttle = LoginThrottle::default();
        let now = Instant::now();
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        fail(&throttle, a, MAX_FAILURES, now);
        assert!(throttle.refused("2001:db8:1:2:ffff::9".parse().unwrap(), now));
        assert!(!throttle.refused("2001:db8:1:3::1".parse().unwrap(), now));
        // An IPv4 address seen as IPv6 by a dual-stack socket is that IPv4 address.
        let mapped: IpAddr = "::ffff:192.0.2.1".parse().unwrap();
        fail(&throttle, mapped, MAX_FAILURES, now);
        assert!(throttle.refused("192.0.2.1".parse().unwrap(), now));
    }

    #[test]
    fn a_full_table_drops_only_ended_windows() {
        let size = u32::try_from(MAX_ENTRIES).unwrap();
        let start = Instant::now();
        // All but one with a window that ends at `now`; the last one refused.
        let throttle = LoginThrottle::default();
        for n in 1..size {
            throttle.failed(v4(n), start);
        }
        let now = start + WINDOW;
        let refused = v4(size);
        fail(&throttle, refused, MAX_FAILURES, now);
        assert_eq!(len(&throttle), MAX_ENTRIES);
        let new = v4(size + 1);
        throttle.failed(new, now);
        assert_eq!(len(&throttle), 2, "the ended windows made room");
        assert!(throttle.refused(refused, now));
        fail(&throttle, new, MAX_FAILURES - 1, now);
        assert!(throttle.refused(new, now), "the new address was recorded");

        // No window has ended: the new address is not recorded, and nobody is forgotten.
        let throttle = LoginThrottle::default();
        fail(&throttle, v4(1), MAX_FAILURES, start);
        for n in 2..=size {
            throttle.failed(v4(n), start);
        }
        assert_eq!(len(&throttle), MAX_ENTRIES);
        fail(&throttle, new, MAX_FAILURES, start);
        assert_eq!(len(&throttle), MAX_ENTRIES);
        assert!(!throttle.refused(new, start));
        assert!(throttle.refused(v4(1), start));
    }

    #[test]
    fn client_address() {
        let peer: IpAddr = "10.0.0.1".parse().unwrap();
        let xff = HeaderName::from_static("x-forwarded-for");
        let headers = |lines: &[&str]| {
            let mut h = HeaderMap::new();
            for line in lines {
                h.append(&xff, HeaderValue::from_str(line).unwrap());
            }
            h
        };
        let chain = headers(&["1.2.3.4, 10.0.0.7"]);
        // Without the setting the header is not looked at.
        assert_eq!(super::client_address(peer, &chain, None), peer);
        assert_eq!(
            super::client_address(peer, &chain, Some(&xff)),
            "10.0.0.7".parse::<IpAddr>().unwrap()
        );
        for unreadable in [headers(&[]), headers(&[""]), headers(&["garbage"])] {
            assert_eq!(super::client_address(peer, &unreadable, Some(&xff)), peer);
        }
        assert_eq!(
            super::client_address(
                peer,
                &headers(&["1.1.1.1", "2.2.2.2, 2001:db8::5"]),
                Some(&xff)
            ),
            "2001:db8::5".parse::<IpAddr>().unwrap()
        );
    }
}
