//! Serde types for the files of a configuration repository.

pub mod host;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ValidationError;

/// Problem severity. Ordered from least to most severe.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    High,
    Critical,
}

/// Validates a group or template name: `^[a-z0-9][a-z0-9_-]{0,62}$`.
pub fn validate_name(s: &str) -> Result<(), ValidationError> {
    let b = s.as_bytes();
    let ok = !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-');
    if ok {
        Ok(())
    } else {
        Err(ValidationError::Name(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_orders_and_parses() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::High);
        assert!(Severity::High < Severity::Critical);
        assert_eq!(
            serde_json::from_str::<Severity>("\"high\"").unwrap(),
            Severity::High
        );
        assert!(serde_json::from_str::<Severity>("\"High\"").is_err());
        assert!(serde_json::from_str::<Severity>("\"error\"").is_err());
    }

    #[test]
    fn validate_name_rules() {
        for ok in ["kunde-a", "region_2", "a"] {
            assert_eq!(validate_name(ok), Ok(()), "{ok}");
        }
        let long = "a".repeat(64);
        for bad in ["", "Kunde", "-a", long.as_str(), "a b"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(validate_name(&"a".repeat(63)), Ok(()));
    }
}
