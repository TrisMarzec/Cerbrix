//! Users, roles and passwords.
//!
//! Passwords are stored only as Argon2id PHC strings; hashing and verifying run on blocking
//! threads because they take tens of milliseconds of CPU and a lot of memory.

use std::fmt;
use std::str::FromStr;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher as _, PasswordVerifier as _};
use rand::RngExt as _;

/// What a user may do. The order is the order of privilege: `Viewer < Operator < Admin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Reads everything.
    Viewer,
    /// Also acknowledges and closes problems.
    Operator,
    /// Also manages users, enrolment keys, host bindings and display tokens.
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "viewer" => Ok(Role::Viewer),
            "operator" => Ok(Role::Operator),
            "admin" => Ok(Role::Admin),
            other => Err(format!(
                "unknown role {other:?}: expected viewer, operator or admin"
            )),
        }
    }
}

/// Shortest accepted password, in characters.
pub const MIN_PASSWORD_CHARS: usize = 12;
/// Longest accepted password, in characters.
pub const MAX_PASSWORD_CHARS: usize = 256;

/// Checks a user name: `^[a-z0-9][a-z0-9._-]{0,62}$`.
pub fn validate_user_name(s: &str) -> Result<(), String> {
    let bad = || {
        format!(
            "invalid user name {s:?}: use 1 to 63 characters from a-z, 0-9, '.', '_' and '-', \
             starting with a letter or digit"
        )
    };
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return Err(bad());
    }
    let ok_first = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let ok_rest = |b: u8| ok_first(b) || matches!(b, b'.' | b'_' | b'-');
    if ok_first(bytes[0]) && bytes[1..].iter().copied().all(ok_rest) {
        Ok(())
    } else {
        Err(bad())
    }
}

/// Checks the length of a password, counted in characters (Unicode scalar values). A
/// password with a NUL character is refused: PostgreSQL and many tools mishandle it. The
/// message never contains the password.
pub fn validate_password(s: &str) -> Result<(), String> {
    let chars = s.chars().count();
    if chars < MIN_PASSWORD_CHARS {
        return Err(format!(
            "the password must have at least {MIN_PASSWORD_CHARS} characters"
        ));
    }
    if chars > MAX_PASSWORD_CHARS {
        return Err(format!(
            "the password must have at most {MAX_PASSWORD_CHARS} characters"
        ));
    }
    if s.contains('\0') {
        return Err("the password must not contain a NUL character".to_owned());
    }
    Ok(())
}

/// Hashes a password with Argon2id (default parameters, random salt) into a PHC string, on
/// a blocking thread.
pub async fn hash_password(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(|e| anyhow::anyhow!("cannot hash the password: {e}"))
    })
    .await?
}

/// Checks a password against a PHC string, on a blocking thread. A malformed hash or a
/// failed worker verifies as false.
pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .verify_password(password.as_bytes(), hash.as_str())
            .is_ok()
    })
    .await
    .unwrap_or(false)
}

/// Generates a password of 20 characters, each drawn uniformly from a-z, A-Z and 0-9 with
/// the cryptographically secure thread generator.
pub fn generate_password() -> String {
    const ALPHABET: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    (0..20)
        .map(|_| char::from(ALPHABET[rng.random_range(0..ALPHABET.len())]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_order_and_text() {
        assert!(Role::Viewer < Role::Operator);
        assert!(Role::Operator < Role::Admin);
        for (text, role) in [
            ("viewer", Role::Viewer),
            ("operator", Role::Operator),
            ("admin", Role::Admin),
        ] {
            assert_eq!(text.parse::<Role>().unwrap(), role);
            assert_eq!(role.as_str(), text);
        }
        assert!("root".parse::<Role>().is_err());
    }

    #[test]
    fn user_name_rules() {
        for ok in ["t.marzec", "ops-1", "a", "0x_y", &"a".repeat(63)] {
            assert!(validate_user_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Admin", "-a", &"a".repeat(64), "a b", "a\n", "ü"] {
            assert!(validate_user_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn password_rules() {
        assert!(validate_password(&"a".repeat(11)).is_err());
        assert!(validate_password(&"a".repeat(12)).is_ok());
        assert!(validate_password(&"a".repeat(256)).is_ok());
        assert!(validate_password(&"a".repeat(257)).is_err());
        // Counted in characters, not bytes.
        assert!(validate_password(&"ü".repeat(12)).is_ok());
        assert!(validate_password(&"ü".repeat(11)).is_err());
        assert!(validate_password(&"ü".repeat(256)).is_ok());
        assert!(validate_password(&"ü".repeat(257)).is_err());
        // NUL is refused; the message does not echo the password.
        let with_nul = format!("{}\0{}", "a".repeat(6), "b".repeat(6));
        let err = validate_password(&with_nul).unwrap_err();
        assert!(err.contains("NUL") && !err.contains("aaaa"), "{err}");
    }

    #[tokio::test]
    async fn hash_and_verify() {
        let password = "correct horse battery".to_owned();
        let a = hash_password(password.clone()).await.unwrap();
        let b = hash_password(password.clone()).await.unwrap();
        assert_ne!(a, b, "the salt is random");
        assert!(a.starts_with("$argon2id$"), "{a}");
        assert!(!a.contains(&password));
        assert!(verify_password(password.clone(), a.clone()).await);
        assert!(verify_password(password.clone(), b).await);
        assert!(!verify_password("wrong horse battery".to_owned(), a).await);
        for malformed in ["", "garbage", "$argon2id$", "$argon2id$v=19$m=1$x$y"] {
            assert!(
                !verify_password(password.clone(), malformed.to_owned()).await,
                "{malformed}"
            );
        }
    }

    #[test]
    fn generated_password_shape() {
        let a = generate_password();
        let b = generate_password();
        assert_eq!(a.chars().count(), 20);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()), "{a}");
        assert_ne!(a, b);
        assert!(validate_password(&a).is_ok());
    }
}
