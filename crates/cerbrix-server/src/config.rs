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
    #[serde(default)]
    pub gitops: GitopsConfig,
    #[serde(default)]
    pub alerting: AlertingConfig,
    #[serde(default)]
    pub smtp: SmtpConfig,
    #[serde(default)]
    pub auth: AuthConfig,
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

/// Settings for trigger evaluation.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AlertingConfig {
    /// Values older than this are stored but never open or close a problem.
    pub max_value_age_secs: u64,
    /// How often the no-data check runs.
    pub nodata_check_secs: u64,
}

impl Default for AlertingConfig {
    fn default() -> Self {
        Self {
            max_value_age_secs: 300,
            nodata_check_secs: 15,
        }
    }
}

impl AlertingConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        let bad = |msg: &str| Err(ConfigError::Invalid(msg.to_owned()));
        if !(10..=86_400).contains(&self.max_value_age_secs) {
            return bad("alerting.max_value_age_secs must be between 10 and 86400");
        }
        if !(5..=300).contains(&self.nodata_check_secs) {
            return bad("alerting.nodata_check_secs must be between 5 and 300");
        }
        Ok(())
    }
}

/// Settings for web UI sessions.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// A session unused for this long ends.
    pub session_idle_secs: u64,
    /// A session ends this long after login, however active it is.
    pub session_max_secs: u64,
    /// Whether the session cookie carries `Secure`. Unset: on, unless `server.insecure_http`.
    /// Behind a TLS-terminating reverse proxy, set it to true.
    pub secure_cookie: Option<bool>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            session_idle_secs: 43_200,
            session_max_secs: 604_800,
            secure_cookie: None,
        }
    }
}

impl AuthConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        let bad = |msg: &str| Err(ConfigError::Invalid(msg.to_owned()));
        if !(300..=604_800).contains(&self.session_idle_secs) {
            return bad("auth.session_idle_secs must be between 300 and 604800");
        }
        if self.session_max_secs < self.session_idle_secs {
            return bad("auth.session_max_secs must not be smaller than auth.session_idle_secs");
        }
        if self.session_max_secs > 2_592_000 {
            return bad("auth.session_max_secs must be at most 2592000");
        }
        Ok(())
    }
}

/// How the connection to the mail server is protected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpEncryption {
    /// Plain connection upgraded with STARTTLS; no fallback to plain text.
    #[default]
    Starttls,
    /// TLS from the first byte (SMTPS).
    Tls,
    /// Plain text. Never combined with credentials.
    None,
}

/// Largest allowed `smtp.timeout_secs`; the dispatcher sizes its batch budget from it.
pub const MAX_SMTP_TIMEOUT_SECS: u64 = 120;

/// Settings for sending notifications by e-mail. Disabled while `host` is unset.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct SmtpConfig {
    pub host: Option<String>,
    pub port: u16,
    pub encryption: SmtpEncryption,
    pub username: Option<String>,
    /// Never printed or logged.
    pub password: Option<String>,
    /// The sender, e.g. `Cerbrix <cerbrix@example.com>`.
    pub from: Option<String>,
    /// Total time allowed for one message, in seconds.
    pub timeout_secs: u64,
}

impl Default for SmtpConfig {
    fn default() -> Self {
        Self {
            host: None,
            port: 587,
            encryption: SmtpEncryption::Starttls,
            username: None,
            password: None,
            from: None,
            timeout_secs: 15,
        }
    }
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("encryption", &self.encryption)
            .field("username", &self.username)
            .field("password", &Redacted(self.password.is_some()))
            .field("from", &self.from)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl SmtpConfig {
    /// E-mail notifications are sent when a host is configured.
    pub fn enabled(&self) -> bool {
        self.host.is_some()
    }

    /// Messages never contain the password or the user name.
    fn validate(&self) -> Result<(), ConfigError> {
        let bad = |msg: &str| Err(ConfigError::Invalid(msg.to_owned()));
        if self.port == 0 {
            return bad("smtp.port must not be 0");
        }
        if !(1..=MAX_SMTP_TIMEOUT_SECS).contains(&self.timeout_secs) {
            return bad(&format!(
                "smtp.timeout_secs must be between 1 and {MAX_SMTP_TIMEOUT_SECS}"
            ));
        }
        let Some(host) = &self.host else {
            return Ok(());
        };
        if host.is_empty() || host.contains(|c: char| c.is_control() || c.is_whitespace()) {
            return bad("smtp.host is not a valid host name");
        }
        match &self.from {
            None => return bad("smtp.from is required when smtp.host is set"),
            Some(from) if from.parse::<lettre::message::Mailbox>().is_err() => {
                return bad("smtp.from is not a valid e-mail address");
            }
            Some(_) => {}
        }
        if self.username.as_deref() == Some("") {
            return bad("smtp.username must not be empty");
        }
        if self.password.as_deref() == Some("") {
            return bad("smtp.password must not be empty");
        }
        if self.password.is_some() && self.username.is_none() {
            return bad("smtp.password requires smtp.username");
        }
        if self.username.is_some() && self.password.is_none() {
            return bad("smtp.username requires smtp.password");
        }
        if self.encryption == SmtpEncryption::None && self.username.is_some() {
            return bad("smtp.encryption = \"none\" would send the password unencrypted");
        }
        Ok(())
    }
}

/// Settings for configuration from a Git repository.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct GitopsConfig {
    pub enabled: bool,
    pub url: Option<String>,
    pub branch: String,
    pub interval_secs: u64,
    /// Access token for an `https://` URL. Never printed or logged.
    pub token: Option<String>,
    pub ssh_key_file: Option<PathBuf>,
    pub known_hosts_file: Option<PathBuf>,
    pub cache_dir: PathBuf,
    pub max_host_removal_percent: u8,
}

impl Default for GitopsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: None,
            branch: "main".to_owned(),
            interval_secs: 60,
            token: None,
            ssh_key_file: None,
            known_hosts_file: None,
            cache_dir: PathBuf::from("/var/lib/cerbrix/git"),
            max_host_removal_percent: 50,
        }
    }
}

/// Prints a placeholder instead of a secret.
struct Redacted(bool);

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0 { "<redacted>" } else { "<unset>" })
    }
}

/// The URL for display: without user name and password.
fn url_without_userinfo(raw: &str) -> String {
    if raw.contains("://") {
        match url::Url::parse(raw) {
            Ok(mut u) => {
                let _ = u.set_username("");
                let _ = u.set_password(None);
                u.to_string()
            }
            Err(_) => "<unparseable>".to_owned(),
        }
    } else {
        // `user@host:path` and local paths: drop everything up to the `@`.
        match raw.split_once('@') {
            Some((_, rest)) => rest.to_owned(),
            None => raw.to_owned(),
        }
    }
}

impl fmt::Debug for GitopsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitopsConfig")
            .field("enabled", &self.enabled)
            .field("url", &self.url.as_deref().map(url_without_userinfo))
            .field("branch", &self.branch)
            .field("interval_secs", &self.interval_secs)
            .field("token", &Redacted(self.token.is_some()))
            .field("ssh_key_file", &self.ssh_key_file)
            .field("known_hosts_file", &self.known_hosts_file)
            .field("cache_dir", &self.cache_dir)
            .field("max_host_removal_percent", &self.max_host_removal_percent)
            .finish()
    }
}

/// The forms of repository URL the Git source accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitUrlKind {
    Https,
    Ssh,
    /// `user@host:path`
    ScpLike,
    /// An absolute path or a `file://` URL.
    Local,
}

const URL_FORMS: &str = "must use https://, ssh://, file://, user@host:path or an absolute path";

/// Classifies a repository URL, or says why it is not acceptable. The text never
/// contains the URL: it may carry credentials.
pub(crate) fn classify_git_url(url: &str) -> Result<GitUrlKind, String> {
    const CREDENTIALS: &str = "contains credentials; put the token in gitops.token";
    if url.is_empty() {
        return Err("must not be empty".into());
    }
    if url.chars().any(char::is_control) {
        return Err("must not contain control characters".into());
    }
    if url.starts_with('-') {
        return Err("must not start with a dash".into());
    }
    if Path::new(url).is_absolute() {
        return Ok(GitUrlKind::Local);
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        return match scheme.to_ascii_lowercase().as_str() {
            "file" if !rest.is_empty() => Ok(GitUrlKind::Local),
            kind @ ("https" | "ssh") => {
                let parsed = url::Url::parse(url).map_err(|_| "is not a valid URL".to_owned())?;
                match parsed.host_str() {
                    Some(host) if !host.is_empty() && !host.starts_with('-') => {}
                    _ => return Err("has no valid host".into()),
                }
                if parsed.password().is_some() || (kind == "https" && !parsed.username().is_empty())
                {
                    return Err(CREDENTIALS.into());
                }
                Ok(if kind == "https" {
                    GitUrlKind::Https
                } else {
                    GitUrlKind::Ssh
                })
            }
            "http" => {
                Err("uses http://, which would send the token unencrypted; use https://".into())
            }
            _ => Err(URL_FORMS.into()),
        };
    }
    if let Some((before_colon, path)) = url.split_once(':')
        && let Some((user, host)) = before_colon.split_once('@')
    {
        let plain = |s: &str| !s.is_empty() && !s.contains(['/', '@', ' ']);
        if plain(user) && plain(host) && !host.starts_with('-') && !path.is_empty() {
            return Ok(GitUrlKind::ScpLike);
        }
    }
    Err(URL_FORMS.into())
}

/// A branch name that is safe to hand to `git fetch` as `refs/heads/<branch>`.
pub(crate) fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with(['-', '/'])
        && !branch.ends_with(['/', '.'])
        && !branch.ends_with(".lock")
        && !branch.contains("..")
        && !branch.contains("//")
        && !branch.contains("@{")
        && !branch.contains(|c: char| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
}

impl GitopsConfig {
    /// Messages never contain the URL or the token.
    fn validate(&self) -> Result<(), ConfigError> {
        let bad = |msg: &str| Err(ConfigError::Invalid(msg.to_owned()));
        if !(5..=3600).contains(&self.interval_secs) {
            return bad("gitops.interval_secs must be between 5 and 3600");
        }
        if !(1..=100).contains(&self.max_host_removal_percent) {
            return bad("gitops.max_host_removal_percent must be between 1 and 100");
        }
        if !self.enabled {
            return Ok(());
        }
        let Some(url) = &self.url else {
            return bad("gitops.url is required when gitops.enabled is true");
        };
        let kind = classify_git_url(url)
            .map_err(|why| ConfigError::Invalid(format!("gitops.url {why}")))?;
        if !valid_branch(&self.branch) {
            return bad("gitops.branch is not a valid branch name");
        }
        if self.token.is_some() && self.ssh_key_file.is_some() {
            return bad("gitops.token and gitops.ssh_key_file are mutually exclusive");
        }
        if self.token.as_deref() == Some("") {
            return bad("gitops.token must not be empty");
        }
        if self.token.is_some() && kind != GitUrlKind::Https {
            return bad("gitops.token requires an https:// gitops.url");
        }
        if self.ssh_key_file.is_some() && !matches!(kind, GitUrlKind::Ssh | GitUrlKind::ScpLike) {
            return bad("gitops.ssh_key_file requires an ssh:// or user@host:path gitops.url");
        }
        for (key, path) in [
            ("gitops.ssh_key_file", &self.ssh_key_file),
            ("gitops.known_hosts_file", &self.known_hosts_file),
        ] {
            if let Some(path) = path
                && (path.as_os_str().is_empty()
                    || path.to_string_lossy().contains(char::is_control))
            {
                return Err(ConfigError::Invalid(format!("{key} is not a valid path")));
            }
            if let Some(path) = path
                && !path.is_absolute()
            {
                return Err(ConfigError::Invalid(format!(
                    "{key} must be an absolute path"
                )));
            }
        }
        if self.cache_dir.as_os_str().is_empty() {
            return bad("gitops.cache_dir must not be empty");
        }
        Ok(())
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
        self.gitops.validate()?;
        self.alerting.validate()?;
        self.smtp.validate()?;
        self.auth.validate()?;
        Ok(())
    }

    /// Whether the session cookie is marked `Secure`.
    pub fn secure_cookie(&self) -> bool {
        self.auth
            .secure_cookie
            .unwrap_or(!self.server.insecure_http)
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

    const GIT_URL: &str = "https://git.example.com/org/config.git";

    fn gitops_toml_with_url(url: &str, extra: &str) -> String {
        format!(
            "{}[gitops]\nenabled = true\nurl = \"{url}\"\n{extra}",
            base_toml(DB, VM)
        )
    }

    fn gitops_toml(extra: &str) -> String {
        gitops_toml_with_url(GIT_URL, extra)
    }

    fn load_gitops(toml: &str) -> Result<Config, ConfigError> {
        std::fs::write("cerbrix.toml", toml).unwrap();
        Config::load(Some(Path::new("cerbrix.toml")))
    }

    #[test]
    fn gitops_defaults() {
        Jail::expect_with(|jail| {
            jail.create_file("cerbrix.toml", &base_toml(DB, VM))?;
            let cfg = Config::load(Some(Path::new("cerbrix.toml"))).unwrap();
            let g = &cfg.gitops;
            assert!(!g.enabled);
            assert_eq!(g.url, None);
            assert_eq!(g.branch, "main");
            assert_eq!(g.interval_secs, 60);
            assert_eq!(g.max_host_removal_percent, 50);
            assert_eq!(g.cache_dir, PathBuf::from("/var/lib/cerbrix/git"));
            assert!(g.token.is_none() && g.ssh_key_file.is_none());
            Ok(())
        });
    }

    #[test]
    fn enabled_requires_url() {
        Jail::expect_with(|jail| {
            let toml = format!("{}[gitops]\nenabled = true\n", base_toml(DB, VM));
            jail.create_file("cerbrix.toml", &toml)?;
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(msg.contains("gitops.url"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn token_and_key_are_exclusive() {
        Jail::expect_with(|_| {
            let msg = invalid(load_gitops(&gitops_toml(&format!(
                "token = \"tok\"\nssh_key_file = \"{ABS_KEY}\"\n"
            ))));
            assert!(
                msg.contains("gitops.token") && msg.contains("gitops.ssh_key_file"),
                "{msg}"
            );
            assert!(!msg.contains("\"tok\""), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn token_requires_https() {
        Jail::expect_with(|_| {
            for url in [
                "ssh://git@host/org/r.git",
                "git@host:org/r.git",
                "/srv/repo",
            ] {
                let msg = invalid(load_gitops(&gitops_toml_with_url(url, "token = \"tok\"\n")));
                assert!(msg.contains("https://"), "{url}: {msg}");
            }
            let msg = invalid(load_gitops(&gitops_toml_with_url(
                "http://host/r.git",
                "token = \"tok\"\n",
            )));
            assert!(msg.contains("unencrypted"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn ssh_key_requires_ssh_url() {
        Jail::expect_with(|_| {
            let key = format!("ssh_key_file = \"{ABS_KEY}\"\n");
            let msg = invalid(load_gitops(&gitops_toml(&key)));
            assert!(msg.contains("gitops.ssh_key_file"), "{msg}");
            for url in ["ssh://git@host/org/r.git", "git@host:org/r.git"] {
                let toml = gitops_toml_with_url(url, &key);
                assert!(load_gitops(&toml).is_ok(), "{url}");
            }
            Ok(())
        });
    }

    /// An absolute path on the platform the tests run on.
    const ABS_KEY: &str = if cfg!(windows) { "C:/k" } else { "/k" };

    #[test]
    fn relative_ssh_key_file_rejected() {
        Jail::expect_with(|_| {
            let toml =
                gitops_toml_with_url("git@host:org/r.git", "ssh_key_file = \"keys/id_ed25519\"\n");
            let msg = invalid(load_gitops(&toml));
            assert!(
                msg.contains("gitops.ssh_key_file") && msg.contains("absolute"),
                "{msg}"
            );
            Ok(())
        });
    }

    #[test]
    fn relative_known_hosts_file_rejected() {
        Jail::expect_with(|_| {
            let toml = gitops_toml_with_url(
                "git@host:org/r.git",
                &format!("ssh_key_file = \"{ABS_KEY}\"\nknown_hosts_file = \"known_hosts\"\n"),
            );
            let msg = invalid(load_gitops(&toml));
            assert!(
                msg.contains("gitops.known_hosts_file") && msg.contains("absolute"),
                "{msg}"
            );
            Ok(())
        });
    }

    #[test]
    fn url_with_password_rejected() {
        Jail::expect_with(|_| {
            for url in [
                "https://user:hunter2@git.example.com/r.git",
                "ssh://user:hunter2@git.example.com/r.git",
            ] {
                let msg = invalid(load_gitops(&gitops_toml_with_url(url, "")));
                assert!(msg.contains("put the token in gitops.token"), "{msg}");
                assert!(!msg.contains("hunter2"), "{msg}");
            }
            Ok(())
        });
    }

    #[test]
    fn dangerous_urls_and_branches_rejected() {
        Jail::expect_with(|_| {
            for url in [
                "-oProxyCommand=evil",
                "--upload-pack=evil",
                "ext::sh -c evil",
                "git://host/r.git",
                "ftp://host/r.git",
                "http://host/r.git",
                "relative/path",
                "git@-oProxyCommand=x:r",
            ] {
                assert!(
                    matches!(
                        load_gitops(&gitops_toml_with_url(url, "")),
                        Err(ConfigError::Invalid(_))
                    ),
                    "{url} accepted"
                );
            }
            for branch in [
                "-x", "a b", "a..b", "a\tb", "a:b", "a~1", "x.lock", "/x", "x/",
            ] {
                let msg = invalid(load_gitops(&gitops_toml(&format!("branch = {branch:?}\n"))));
                assert!(msg.contains("gitops.branch"), "{branch}: {msg}");
            }
            assert!(load_gitops(&gitops_toml("branch = \"release/2026\"\n")).is_ok());
            Ok(())
        });
    }

    #[test]
    fn local_paths_and_file_urls_are_accepted() {
        Jail::expect_with(|_| {
            assert!(load_gitops(&gitops_toml_with_url("file:///srv/repo", "")).is_ok());
            #[cfg(unix)]
            assert!(load_gitops(&gitops_toml_with_url("/srv/repo", "")).is_ok());
            #[cfg(windows)]
            assert!(load_gitops(&gitops_toml_with_url("C:/repo", "")).is_ok());
            Ok(())
        });
    }

    #[test]
    fn alerting_config_defaults_and_bounds() {
        Jail::expect_with(|_| {
            let cfg = load_gitops(&base_toml(DB, VM)).unwrap();
            assert_eq!(cfg.alerting.max_value_age_secs, 300);
            assert_eq!(cfg.alerting.nodata_check_secs, 15);
            let with =
                |extra: &str| load_gitops(&format!("{}[alerting]\n{extra}", base_toml(DB, VM)));
            for (value, ok) in [(9, false), (10, true), (86_400, true), (86_401, false)] {
                let result = with(&format!("max_value_age_secs = {value}\n"));
                assert_eq!(result.is_ok(), ok, "max_value_age_secs {value}");
                if !ok {
                    assert!(invalid(result).contains("alerting.max_value_age_secs"));
                }
            }
            for (value, ok) in [(4, false), (5, true), (300, true), (301, false)] {
                let result = with(&format!("nodata_check_secs = {value}\n"));
                assert_eq!(result.is_ok(), ok, "nodata_check_secs {value}");
                if !ok {
                    assert!(invalid(result).contains("alerting.nodata_check_secs"));
                }
            }
            Ok(())
        });
    }

    #[test]
    fn auth_defaults_and_bounds() {
        Jail::expect_with(|_| {
            let cfg = load_gitops(&base_toml(DB, VM)).unwrap();
            assert_eq!(cfg.auth.session_idle_secs, 43_200);
            assert_eq!(cfg.auth.session_max_secs, 604_800);
            assert_eq!(cfg.auth.secure_cookie, None);
            // Unset, the cookie follows the server mode: base_toml serves plain HTTP.
            assert!(!cfg.secure_cookie());
            let with = |extra: &str| load_gitops(&format!("{}[auth]\n{extra}", base_toml(DB, VM)));
            for (value, ok) in [(299, false), (300, true), (604_800, true), (604_801, false)] {
                let result = with(&format!(
                    "session_idle_secs = {value}\nsession_max_secs = 604800\n"
                ));
                assert_eq!(result.is_ok(), ok, "idle {value}");
                if !ok {
                    assert!(invalid(result).contains("auth.session_idle_secs"));
                }
            }
            // The maximum may not be below the idle time, and has an upper bound.
            let result = with("session_idle_secs = 7200\nsession_max_secs = 7199\n");
            assert!(invalid(result).contains("auth.session_max_secs"));
            assert!(with("session_idle_secs = 7200\nsession_max_secs = 7200\n").is_ok());
            assert!(with("session_max_secs = 2592000\n").is_ok());
            assert!(invalid(with("session_max_secs = 2592001\n")).contains("session_max_secs"));
            // An explicit setting wins over the server mode, both ways.
            assert!(with("secure_cookie = true\n").unwrap().secure_cookie());
            let tls_toml = "[server]\ninsecure_http = false\ntls_cert = \"c.pem\"\ntls_key = \"k.pem\"\n[database]\nurl = \"postgres://localhost/x\"\n[metrics]\nurl = \"http://localhost:8428\"\n";
            let secure_by_default = load_gitops(tls_toml).unwrap();
            assert!(secure_by_default.secure_cookie());
            let off = load_gitops(&format!("{tls_toml}[auth]\nsecure_cookie = false\n")).unwrap();
            assert!(!off.secure_cookie());
            Ok(())
        });
    }

    #[test]
    fn interval_bounds() {
        Jail::expect_with(|_| {
            for (value, ok) in [(4, false), (5, true), (3600, true), (3601, false)] {
                let result = load_gitops(&gitops_toml(&format!("interval_secs = {value}\n")));
                assert_eq!(result.is_ok(), ok, "interval {value}");
                if !ok {
                    assert!(invalid(result).contains("gitops.interval_secs"));
                }
            }
            for (value, ok) in [(0, false), (1, true), (100, true), (101, false)] {
                let toml = gitops_toml(&format!("max_host_removal_percent = {value}\n"));
                assert_eq!(load_gitops(&toml).is_ok(), ok, "percent {value}");
            }
            Ok(())
        });
    }

    #[test]
    fn debug_redacts_token() {
        Jail::expect_with(|_| {
            let cfg = load_gitops(&gitops_toml("token = \"ghp_SECRETTOKEN\"\n")).unwrap();
            let shown = format!("{cfg:?} {:?}", cfg.gitops);
            assert!(!shown.contains("ghp_SECRETTOKEN"), "{shown}");
            assert!(shown.contains("token: <redacted>"), "{shown}");
            Ok(())
        });
    }

    #[test]
    fn debug_hides_url_userinfo() {
        let cfg = GitopsConfig {
            url: Some("https://user:hunter2@git.example.com/r.git".into()),
            ..GitopsConfig::default()
        };
        let shown = format!("{cfg:?}");
        assert!(
            !shown.contains("hunter2") && !shown.contains("user"),
            "{shown}"
        );
        assert!(shown.contains("git.example.com"), "{shown}");
    }

    #[test]
    fn token_from_environment() {
        Jail::expect_with(|jail| {
            jail.create_file("cerbrix.toml", &gitops_toml(""))?;
            jail.set_env("CERBRIX_GITOPS__TOKEN", "envtoken123");
            let cfg = Config::load(Some(Path::new("cerbrix.toml"))).unwrap();
            assert_eq!(cfg.gitops.token.as_deref(), Some("envtoken123"));
            assert!(!format!("{cfg:?}").contains("envtoken123"));
            Ok(())
        });
    }

    #[test]
    fn token_is_not_echoed_in_errors() {
        Jail::expect_with(|jail| {
            // Wrong type.
            let msg = invalid(load_gitops(&gitops_toml("token = 8675309\n")));
            assert!(!msg.contains("8675309"), "{msg}");
            assert!(msg.contains("gitops.token"), "{msg}");
            // Syntax error on the token line.
            let msg = invalid(load_gitops(&gitops_toml("token = \"ghp_SECRETTOKEN\n")));
            assert!(!msg.contains("ghp_SECRETTOKEN"), "{msg}");
            // Wrong type through the environment.
            jail.create_file("cerbrix.toml", &gitops_toml(""))?;
            jail.set_env("CERBRIX_GITOPS__INTERVAL_SECS", "ENVSECRET");
            jail.set_env("CERBRIX_GITOPS__TOKEN", "ghp_ENVTOKEN");
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(
                !msg.contains("ENVSECRET") && !msg.contains("ghp_ENVTOKEN"),
                "{msg}"
            );
            // A validation failure while a token is set.
            jail.set_env("CERBRIX_GITOPS__INTERVAL_SECS", "1");
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(!msg.contains("ghp_ENVTOKEN"), "{msg}");
            Ok(())
        });
    }

    fn smtp_toml(extra: &str) -> String {
        format!(
            "{}[smtp]\nhost = \"mail.example.com\"\nfrom = \"cerbrix@example.com\"\n{extra}",
            base_toml(DB, VM)
        )
    }

    #[test]
    fn smtp_disabled_by_default() {
        Jail::expect_with(|jail| {
            jail.create_file("cerbrix.toml", &base_toml(DB, VM))?;
            let cfg = Config::load(Some(Path::new("cerbrix.toml"))).unwrap();
            assert!(!cfg.smtp.enabled());
            assert_eq!(cfg.smtp.port, 587);
            assert_eq!(cfg.smtp.encryption, SmtpEncryption::Starttls);
            assert_eq!(cfg.smtp.timeout_secs, 15);
            let cfg = load_gitops(&smtp_toml("")).unwrap();
            assert!(cfg.smtp.enabled());
            Ok(())
        });
    }

    #[test]
    fn smtp_requires_from() {
        Jail::expect_with(|_| {
            let toml = format!("{}[smtp]\nhost = \"mail.example.com\"\n", base_toml(DB, VM));
            let msg = invalid(load_gitops(&toml));
            assert!(msg.contains("smtp.from"), "{msg}");
            let toml = format!(
                "{}[smtp]\nhost = \"mail.example.com\"\nfrom = \"not an address\"\n",
                base_toml(DB, VM)
            );
            let msg = invalid(load_gitops(&toml));
            assert!(msg.contains("smtp.from"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn smtp_password_requires_username() {
        Jail::expect_with(|_| {
            let msg = invalid(load_gitops(&smtp_toml("password = \"pw-SECRET\"\n")));
            assert!(
                msg.contains("smtp.password requires smtp.username"),
                "{msg}"
            );
            assert!(!msg.contains("pw-SECRET"), "{msg}");
            let msg = invalid(load_gitops(&smtp_toml("username = \"bob\"\n")));
            assert!(
                msg.contains("smtp.username requires smtp.password"),
                "{msg}"
            );
            let msg = invalid(load_gitops(&smtp_toml(
                "username = \"bob\"\npassword = \"\"\n",
            )));
            assert!(msg.contains("smtp.password must not be empty"), "{msg}");
            let ok = smtp_toml("username = \"bob\"\npassword = \"pw\"\n");
            assert!(load_gitops(&ok).is_ok());
            Ok(())
        });
    }

    #[test]
    fn smtp_none_with_credentials_rejected() {
        Jail::expect_with(|_| {
            let toml =
                smtp_toml("encryption = \"none\"\nusername = \"bob\"\npassword = \"pw-SECRET\"\n");
            let msg = invalid(load_gitops(&toml));
            assert!(
                msg.contains("smtp.encryption = \"none\" would send the password unencrypted"),
                "{msg}"
            );
            assert!(!msg.contains("pw-SECRET"), "{msg}");
            assert!(load_gitops(&smtp_toml("encryption = \"none\"\n")).is_ok());
            assert!(load_gitops(&smtp_toml("encryption = \"tls\"\n")).is_ok());
            let msg = invalid(load_gitops(&smtp_toml("encryption = \"ssl\"\n")));
            assert!(msg.contains("encryption"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn smtp_debug_redacts_password() {
        Jail::expect_with(|_| {
            let toml = smtp_toml("username = \"bob\"\npassword = \"pw-SECRET\"\n");
            let cfg = load_gitops(&toml).unwrap();
            let shown = format!("{cfg:?} {:?}", cfg.smtp);
            assert!(!shown.contains("pw-SECRET"), "{shown}");
            assert!(shown.contains("password: <redacted>"), "{shown}");
            Ok(())
        });
    }

    #[test]
    fn smtp_password_is_not_echoed_in_errors() {
        Jail::expect_with(|jail| {
            // Wrong type.
            let msg = invalid(load_gitops(&smtp_toml("password = 8675309\n")));
            assert!(!msg.contains("8675309"), "{msg}");
            assert!(msg.contains("smtp.password"), "{msg}");
            // Syntax error on the password line.
            let msg = invalid(load_gitops(&smtp_toml("password = \"pw-SECRET\n")));
            assert!(!msg.contains("pw-SECRET"), "{msg}");
            // Wrong type elsewhere while the password comes from the environment.
            jail.create_file("cerbrix.toml", &smtp_toml("username = \"bob\"\n"))?;
            jail.set_env("CERBRIX_SMTP__PASSWORD", "pw-ENVSECRET");
            jail.set_env("CERBRIX_SMTP__PORT", "ENVSECRET");
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(
                !msg.contains("ENVSECRET") && !msg.contains("pw-ENVSECRET"),
                "{msg}"
            );
            // A validation failure while the password is set.
            jail.set_env("CERBRIX_SMTP__PORT", "0");
            let msg = invalid(Config::load(Some(Path::new("cerbrix.toml"))));
            assert!(msg.contains("smtp.port"), "{msg}");
            assert!(!msg.contains("pw-ENVSECRET"), "{msg}");
            Ok(())
        });
    }

    #[test]
    fn smtp_password_from_environment() {
        Jail::expect_with(|jail| {
            jail.create_file("cerbrix.toml", &smtp_toml("username = \"bob\"\n"))?;
            jail.set_env("CERBRIX_SMTP__PASSWORD", "pw-ENVSECRET");
            let cfg = Config::load(Some(Path::new("cerbrix.toml"))).unwrap();
            assert_eq!(cfg.smtp.password.as_deref(), Some("pw-ENVSECRET"));
            assert!(!format!("{cfg:?}").contains("pw-ENVSECRET"));
            Ok(())
        });
    }

    #[test]
    fn smtp_timeout_bounds() {
        Jail::expect_with(|_| {
            for (value, ok) in [(0, false), (1, true), (120, true), (121, false)] {
                let toml = smtp_toml(&format!("timeout_secs = {value}\n"));
                assert_eq!(load_gitops(&toml).is_ok(), ok, "timeout {value}");
            }
            Ok(())
        });
    }
}
