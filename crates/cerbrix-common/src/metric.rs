//! Metric values and their validation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_BATCH_VALUES: usize = 5000;
pub const MAX_PARAMS: usize = 8;
pub const MAX_PARAM_VALUE_LEN: usize = 256;
pub const MAX_ITEM_KEY_LEN: usize = 128;

/// One measurement of one item on one host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricValue {
    /// Item key, e.g. `disk.used_percent`.
    pub key: String,
    /// Item parameters, e.g. `{"mount": "/"}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// Measurement time in Unix milliseconds.
    pub ts_ms: i64,
    pub value: f64,
}

impl MetricValue {
    /// Checks key, parameters and value against the wire rules.
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_item_key(&self.key)?;
        if self.params.len() > MAX_PARAMS {
            return Err(ValidationError::TooManyParams);
        }
        for (name, val) in &self.params {
            if !is_identifier(name) {
                return Err(ValidationError::ParamName(name.clone()));
            }
            if val.is_empty()
                || val.len() > MAX_PARAM_VALUE_LEN
                || val.chars().any(char::is_control)
            {
                return Err(ValidationError::ParamValue(name.clone()));
            }
        }
        if !self.value.is_finite() {
            return Err(ValidationError::NotFinite);
        }
        Ok(())
    }
}

/// A batch of values sent by an agent in one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueBatch {
    pub protocol_version: u32,
    pub values: Vec<MetricValue>,
}

/// Server answer to a value batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestResponse {
    pub accepted: usize,
    pub rejected: usize,
}

/// Item keys are at most `MAX_ITEM_KEY_LEN` bytes of dot-separated identifiers:
/// `^[a-z][a-z0-9_]*(.[a-z][a-z0-9_]*)*$`.
pub fn validate_item_key(key: &str) -> Result<(), ValidationError> {
    if key.len() <= MAX_ITEM_KEY_LEN && key.split('.').all(is_identifier) {
        Ok(())
    } else {
        Err(ValidationError::ItemKey(key.to_owned()))
    }
}

/// Matches `^[a-z][a-z0-9_]*$`.
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn value(key: &str, v: f64) -> MetricValue {
        MetricValue {
            key: key.into(),
            params: BTreeMap::new(),
            ts_ms: 1_700_000_000_000,
            value: v,
        }
    }
    fn value_with_param(key: &str, name: &str, val: &str) -> MetricValue {
        let mut m = value(key, 1.0);
        m.params.insert(name.into(), val.into());
        m
    }
    fn value_with_n_params(n: usize) -> MetricValue {
        let mut m = value("disk.used_percent", 1.0);
        for i in 0..n {
            m.params.insert(format!("p{i}"), "x".into());
        }
        m
    }

    #[test]
    fn value_roundtrips_through_json_without_empty_params() {
        let v = MetricValue {
            key: "cpu.usage".into(),
            params: BTreeMap::new(),
            ts_ms: 1_700_000_000_000,
            value: 12.5,
        };
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(
            json,
            r#"{"key":"cpu.usage","ts_ms":1700000000000,"value":12.5}"#
        );
        assert_eq!(serde_json::from_str::<MetricValue>(&json).unwrap(), v);
    }
    #[test]
    fn odd_mount_points_roundtrip() {
        for mount in [
            "/mnt/my disk",
            r#"/mnt/"quoted""#,
            r"C:\Data",
            "/mnt/übung/日本",
        ] {
            let v = value_with_param("disk.used_percent", "mount", mount);
            assert!(v.validate().is_ok());
            let back: MetricValue =
                serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
            assert_eq!(back.params["mount"], mount);
        }
    }
    #[test]
    fn item_key_length_limit() {
        let at_limit = format!("a{}", "b".repeat(MAX_ITEM_KEY_LEN - 1));
        assert_eq!(at_limit.len(), 128);
        assert!(validate_item_key(&at_limit).is_ok());
        let over = format!("{at_limit}b");
        assert!(matches!(
            validate_item_key(&over),
            Err(ValidationError::ItemKey(_))
        ));
    }

    #[test]
    fn rejects_non_finite_values() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                value("cpu.usage", bad).validate(),
                Err(ValidationError::NotFinite)
            );
        }
    }
    #[test]
    fn rejects_bad_keys_and_params() {
        assert!(validate_item_key("CPU.usage").is_err());
        assert!(validate_item_key("cpu..usage").is_err());
        assert!(validate_item_key("").is_err());
        assert!(
            value_with_param("disk.used_percent", "Mount", "/")
                .validate()
                .is_err()
        );
        assert!(
            value_with_param("disk.used_percent", "mount", "a\nb")
                .validate()
                .is_err()
        );
        assert!(
            value_with_param("disk.used_percent", "mount", &"x".repeat(257))
                .validate()
                .is_err()
        );
        assert_eq!(
            value_with_n_params(9).validate(),
            Err(ValidationError::TooManyParams)
        );
    }
}
