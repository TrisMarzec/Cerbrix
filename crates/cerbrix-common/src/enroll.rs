//! Host enrolment messages and host name validation.

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollRequest {
    pub host: String,
    pub enrollment_key: String,
    pub agent_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollResponse {
    pub credential: String,
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
    fn host_names() {
        for ok in ["web-01", "db.internal.example", "A1"] {
            assert!(validate_host_name(ok).is_ok());
        }
        for bad in ["", "-web", "web 01", "web/01", &"a".repeat(254)] {
            assert!(validate_host_name(bad).is_err());
        }
    }
}
