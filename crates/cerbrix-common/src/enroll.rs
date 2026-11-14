//! Host enrolment messages and host name validation.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;

#[derive(Clone, Serialize, Deserialize)]
pub struct EnrollRequest {
    pub host: String,
    pub enrollment_key: String,
    pub agent_version: String,
}

// Manual Debug: the enrolment key must never reach logs.
impl fmt::Debug for EnrollRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnrollRequest")
            .field("host", &self.host)
            .field("enrollment_key", &"<redacted>")
            .field("agent_version", &self.agent_version)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct EnrollResponse {
    pub credential: String,
}

// Manual Debug: the credential must never reach logs.
impl fmt::Debug for EnrollResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnrollResponse")
            .field("credential", &"<redacted>")
            .finish()
    }
}

/// Host names match `^[A-Za-z0-9][A-Za-z0-9._-]{0,252}$`.
pub fn validate_host_name(name: &str) -> Result<(), ValidationError> {
    let mut chars = name.chars();
    let ok = name.len() <= 253
        && matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(ValidationError::HostName(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_redacts_secrets() {
        let req = EnrollRequest {
            host: "web-01".into(),
            enrollment_key: "s3cret-key".into(),
            agent_version: "1.2.3".into(),
        };
        let out = format!("{req:?}");
        assert!(!out.contains("s3cret-key"));
        assert!(out.contains("web-01") && out.contains("1.2.3"));
        let resp = EnrollResponse {
            credential: "s3cret-cred".into(),
        };
        let out = format!("{resp:?}");
        assert!(!out.contains("s3cret-cred"));
        assert!(out.contains("<redacted>"));
    }

    #[test]
    fn host_names() {
        for ok in ["web-01", "db.internal.example", "A1"] {
            assert!(validate_host_name(ok).is_ok());
        }
        for bad in ["", "-web", "web 01", "web/01", &"a".repeat(254)] {
            assert!(validate_host_name(bad).is_err());
        }
    }
}
