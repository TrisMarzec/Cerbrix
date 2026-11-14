//! Server configuration: defaults, then a TOML file, then `CERBRIX_*` environment variables.

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;

/// Errors raised while loading the configuration. Messages never contain secret values.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read configuration: {0}")]
    Read(String),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub metrics: MetricsConfig,
    #[serde(default)]
    pub ingest: IngestConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    #[serde(default)]
    pub insecure_http: bool,
}

#[derive(Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

/// VictoriaMetrics base URL.
#[derive(Clone, Deserialize)]
pub struct MetricsConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestConfig {
    #[serde(default = "default_max_future_skew_secs")]
    pub max_future_skew_secs: u64,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            max_future_skew_secs: default_max_future_skew_secs(),
            max_body_bytes: default_max_body_bytes(),
        }
    }
}

fn default_listen() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 8443))
}

fn default_max_future_skew_secs() -> u64 {
    300
}

fn default_max_body_bytes() -> usize {
    2 * 1024 * 1024
}

/// Formats a URL for `Debug` output with the password masked.
fn fmt_redacted(name: &str, raw: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let shown = match url::Url::parse(raw) {
        Ok(mut u) => {
            if u.password().is_some() {
                let _ = u.set_password(Some("***"));
            }
            u.to_string()
        }
        Err(_) => "<unparseable>".to_string(),
    };
    f.debug_struct(name).field("url", &shown).finish()
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_redacted("DatabaseConfig", &self.url, f)
    }
}

impl fmt::Debug for MetricsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_redacted("MetricsConfig", &self.url, f)
    }
}

impl Config {
    /// Loads the configuration. Later sources win: defaults, the TOML file at `path`
    /// (if given), then environment variables such as `CERBRIX_DATABASE__URL`.
    pub fn load(path: Option<&Path>) -> Result<Config, ConfigError> {
        let mut figment = Figment::new();
        if let Some(path) = path {
            std::fs::metadata(path)
                .map_err(|e| ConfigError::Read(format!("{}: {e}", path.display())))?;
            figment = figment.merge(Toml::file(path));
        }
        figment = figment.merge(Env::prefixed("CERBRIX_").split("__"));

        let config: Config = figment
            .extract()
            .map_err(|e| ConfigError::Invalid(describe_figment_error(e)))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let s = &self.server;
        match (&s.tls_cert, &s.tls_key) {
            (Some(_), None) => {
                return Err(ConfigError::Invalid(
                    "server.tls_cert is set but server.tls_key is missing".into(),
                ));
            }
            (None, Some(_)) => {
                return Err(ConfigError::Invalid(
                    "server.tls_key is set but server.tls_cert is missing".into(),
                ));
            }
            (None, None) if !s.insecure_http => {
                return Err(ConfigError::Invalid(
                    "TLS is not configured: set server.tls_cert and server.tls_key, \
                     or set server.insecure_http = true"
                        .into(),
                ));
            }
            _ => {}
        }
        check_url("database.url", &self.database.url)?;
        check_url("metrics.url", &self.metrics.url)?;
        Ok(())
    }
}

/// The offending value is never echoed: database URLs carry passwords.
fn check_url(key: &str, value: &str) -> Result<(), ConfigError> {
    if value.is_empty() {
        return Err(ConfigError::Invalid(format!("{key} must not be empty")));
    }
    if url::Url::parse(value).is_err() {
        return Err(ConfigError::Invalid(format!("{key} is not a valid URL")));
    }
    Ok(())
}
/// Describes a figment error without echoing configured values. The error's own
/// `Display` cannot be used: TOML syntax errors quote the offending source line
/// and type errors print the value, and those may be secrets. Only the kind of
/// problem, the key path and the source (file path) are reported.
fn describe_figment_error(error: figment::Error) -> String {
    error
        .into_iter()
        .map(|e| describe_one(&e))
        .collect::<Vec<_>>()
        .join("; ")
}

fn describe_one(e: &figment::Error) -> String {
    use figment::error::Kind;

    let mut out = match &e.kind {
        Kind::Message(msg) => match toml_error_position(msg) {
            Some((line, column)) => format!("invalid TOML syntax at line {line}, column {column}"),
            None => "invalid value".to_owned(),
        },
        Kind::InvalidType(_, expected) => format!("wrong type, expected {expected}"),
        Kind::InvalidValue(_, expected) => format!("invalid value, expected {expected}"),
        Kind::InvalidLength(_, expected) => format!("invalid length, expected {expected}"),
        // These name keys, never values.
        Kind::UnknownField(..) | Kind::MissingField(_) | Kind::DuplicateField(_) => {
            e.kind.to_string()
        }
        Kind::UnknownVariant(..) => "unknown variant".to_owned(),
        Kind::ISizeOutOfRange(_) | Kind::USizeOutOfRange(_) => "number out of range".to_owned(),
        _ => "unsupported value".to_owned(),
    };
    if !e.path.is_empty() {
        out.push_str(&format!(" for key {:?}", e.path.join(".")));
    }
    if let Some(md) = &e.metadata {
        match &md.source {
            Some(source) => out.push_str(&format!(" in {source} {}", md.name)),
            None => out.push_str(&format!(" in {}", md.name)),
        }
    }
    out
}

/// Line and column from the first line of a TOML parse error message
/// (`TOML parse error at line 3, column 38`); the rest quotes the source.
fn toml_error_position(message: &str) -> Option<(usize, usize)> {
    let first = message
        .lines()
        .next()?
        .strip_prefix("TOML parse error at line ")?;
    let (line, column) = first.split_once(", column ")?;
    Some((line.parse().ok()?, column.trim().parse().ok()?))
}

#[cfg(test)]
// `figment::Jail` closures must return `figment::Error`, which is large.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;
    use figment::Jail;
    use std::net::SocketAddr;
    use std::path::Path;

    const DB: &str = "postgres://cerbrix:s3cret@localhost:5432/cerbrix";
    const VM: &str = "http://localhost:8428";

    fn base_toml(db: &str, metrics: &str) -> String {
        format!(
            "[server]\ninsecure_http = true\n[database]\nurl = \"{db}\"\n[metrics]\nurl = \"{metrics}\"\n"
        )
    }

    fn invalid(result: Result<Config, ConfigError>) -> String {
        match result {
            Err(ConfigError::Invalid(msg)) => msg,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn loads_file_and_env_overrides_file() {
        Jail::expect_with(|jail| {
            jail.create_file("cerbrix.toml", &base_toml("postgres://a@host/a", VM))?;
            jail.set_env("CERBRIX_DATABASE__URL", "postgres://b@host/b");
            let cfg = Config::load(Some(Path::new("cerbrix.toml"))).unwrap();
            assert_eq!(cfg.database.url, "postgres://b@host/b");
            assert_eq!(cfg.metrics.url, VM);
            Ok(())
        });
    }

    #[test]
    fn defaults_apply() {
        Jail::expect_with(|jail| {
            jail.create_file("cerbrix.toml", &base_toml(DB, VM))?;
            let cfg = Config::load(Some(Path::new("cerbrix.toml"))).unwrap();
            assert_eq!(
                cfg.server.listen,
                "0.0.0.0:8443".parse::<SocketAddr>().unwrap()
            );
            assert_eq!(cfg.ingest.max_future_skew_secs, 300);
            assert_eq!(cfg.ingest.max_body_bytes, 2 * 1024 * 1024);
            Ok(())
        });
    }

    #[test]
    fn rejects_missing_tls_without_insecure_flag() {
        Jail::expect_with(|jail| {
            let toml = base_toml(DB, VM).replace("insecure_http = true", "insecure_http = false");
            jail.create_file("cerbrix.toml", &toml)?;
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(msg.contains("server.insecure_http"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn rejects_cert_without_key() {
        Jail::expect_with(|jail| {
            let toml = base_toml(DB, VM).replace("insecure_http = true", "tls_cert = \"cert.pem\"");
            jail.create_file("cerbrix.toml", &toml)?;
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(msg.contains("server.tls_key"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn rejects_unparseable_urls() {
        Jail::expect_with(|jail| {
            jail.create_file("cerbrix.toml", &base_toml(DB, "not a url"))?;
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(msg.contains("metrics.url"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn debug_hides_database_password() {
        Jail::expect_with(|jail| {
            jail.create_file("cerbrix.toml", &base_toml(DB, VM))?;
            let cfg = Config::load(Some(Path::new("cerbrix.toml"))).unwrap();
            assert!(!format!("{cfg:?}").contains("s3cret"));
            Ok(())
        });
    }

    #[test]
    fn invalid_database_url_error_does_not_leak_value() {
        Jail::expect_with(|jail| {
            jail.create_file("cerbrix.toml", &base_toml("s3cret not a url", VM))?;
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(msg.contains("database.url"), "{msg}");
            assert!(!msg.contains("s3cret"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn syntax_error_does_not_echo_the_secret_line() {
        Jail::expect_with(|jail| {
            let toml = "[server]\ninsecure_http = true\n[database]\nurl = \"postgres://cerbrix:SECRETPW@db/cerbrix\n[metrics]\nurl = \"http://vm\"\n";
            jail.create_file("cerbrix.toml", toml)?;
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(!msg.contains("SECRETPW"), "{msg}");
            assert!(!msg.contains("url ="), "{msg}");
            assert!(msg.contains("cerbrix.toml"), "{msg}");
            assert!(msg.contains("line 4"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn wrong_typed_values_are_not_echoed() {
        Jail::expect_with(|jail| {
            let toml = "[server]\ninsecure_http = true\n[database]\nurl = 99999\n[metrics]\nurl = \"http://vm\"\n";
            jail.create_file("cerbrix.toml", toml)?;
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(!msg.contains("99999"), "{msg}");
            assert!(
                msg.contains("database.url") && msg.contains("cerbrix.toml"),
                "{msg}"
            );
            jail.set_env("CERBRIX_SERVER__LISTEN", "ENVSECRET");
            jail.create_file("cerbrix.toml", &base_toml(DB, VM))?;
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(!msg.contains("ENVSECRET"), "{msg}");
            Ok(())
        });
    }
}
