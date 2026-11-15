//! Error types shared across the wire protocol.

use serde::{Deserialize, Serialize};

/// Why a host name, item key, parameter or value was rejected.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ValidationError {
    #[error(
        "invalid host name {0:?}: use 1 to 253 letters, digits, '.', '_' or '-', starting with a letter or digit"
    )]
    HostName(String),
    #[error(
        "invalid item key {0:?}: use dot-separated lowercase segments of letters, digits and '_', each starting with a letter"
    )]
    ItemKey(String),
    #[error(
        "invalid parameter name {0:?}: use lowercase letters, digits and '_', starting with a letter"
    )]
    ParamName(String),
    #[error(
        "invalid value for parameter {0:?}: must be 1 to 256 bytes of UTF-8 without control characters"
    )]
    ParamValue(String),
    #[error(
        "invalid name {0:?}: use 1 to 63 lowercase letters, digits, '_' or '-', starting with a letter or digit"
    )]
    Name(String),
    #[error("too many parameters: a metric value may carry at most 8")]
    TooManyParams,
    #[error("metric value is not a finite number (NaN and infinity are not accepted)")]
    NotFinite,
}

/// JSON body of every error response. `error` is a stable machine-readable code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_offending_input() {
        let msg = ValidationError::ItemKey("CPU.usage".into()).to_string();
        assert!(msg.contains("CPU.usage") && msg.contains("lowercase"));
        assert!(
            ValidationError::TooManyParams
                .to_string()
                .contains("at most 8")
        );
        assert!(ValidationError::NotFinite.to_string().contains("finite"));
    }

    #[test]
    fn error_body_roundtrips() {
        let b = ErrorBody {
            error: "bad_request".into(),
            message: "nope".into(),
        };
        let back: ErrorBody = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(back, b);
    }
}
