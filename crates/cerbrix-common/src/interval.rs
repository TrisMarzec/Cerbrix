//! Collection intervals written as `<digits><s|m|h|d>`.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Why an interval string was rejected.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum IntervalError {
    #[error(
        "invalid interval {0:?}: use digits followed by s, m, h or d, for example \"30s\" or \"5m\""
    )]
    Format(String),
    #[error("interval {0:?} is out of range: use between 1s and 24h")]
    Range(String),
}

/// A collection interval between 1 second and 24 hours.
///
/// Serialises as the shortest exact form: 90 s stays `"90s"`, 300 s is `"5m"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval(Duration);

impl Interval {
    pub const MIN: Duration = Duration::from_secs(1);
    pub const MAX: Duration = Duration::from_secs(24 * 3600);

    pub fn as_secs(&self) -> u64 {
        self.0.as_secs()
    }
}

impl FromStr for Interval {
    type Err = IntervalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let format = || IntervalError::Format(s.to_string());
        let factor: u64 = match s.as_bytes().last() {
            Some(b's') => 1,
            Some(b'm') => 60,
            Some(b'h') => 3600,
            Some(b'd') => 86400,
            _ => return Err(format()),
        };
        let digits = &s[..s.len() - 1];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format());
        }
        let secs = digits
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(factor))
            .ok_or_else(|| IntervalError::Range(s.to_string()))?;
        let d = Duration::from_secs(secs);
        if d < Self::MIN || d > Self::MAX {
            return Err(IntervalError::Range(s.to_string()));
        }
        Ok(Interval(d))
    }
}

impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.as_secs();
        if s.is_multiple_of(86400) {
            write!(f, "{}d", s / 86400)
        } else if s.is_multiple_of(3600) {
            write!(f, "{}h", s / 3600)
        } else if s.is_multiple_of(60) {
            write!(f, "{}m", s / 60)
        } else {
            write!(f, "{s}s")
        }
    }
}

impl Serialize for Interval {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Interval {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Interval {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Interval")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[0-9]+[smhd]$",
            "description": "Collection interval between 1s and 24h, for example \"30s\", \"5m\" or \"1h\"."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_parses_units() {
        for (s, n) in [("30s", 30), ("5m", 300), ("1h", 3600), ("1d", 86400)] {
            assert_eq!(s.parse::<Interval>().unwrap().as_secs(), n, "{s}");
        }
    }

    #[test]
    fn interval_rejects() {
        for s in ["0s", "25h", "2d", "5", "5 m", "1.5h", "-5s", "", "5M"] {
            assert!(s.parse::<Interval>().is_err(), "{s:?} should be rejected");
        }
    }

    #[test]
    fn interval_round_trips() {
        for s in ["5m", "90s"] {
            let i: Interval = serde_json::from_str(&format!("\"{s}\"")).unwrap();
            assert_eq!(serde_json::to_string(&i).unwrap(), format!("\"{s}\""));
        }
    }

    #[test]
    fn interval_schema_is_a_pattern_string() {
        let schema = schemars::schema_for!(Interval);
        let v = serde_json::to_value(&schema).unwrap();
        assert_eq!(v["type"], "string");
        assert_eq!(v["pattern"], "^[0-9]+[smhd]$");
    }
}
