//! Agent configuration: defaults, then a TOML file, then `CERBRIX_AGENT_*` environment variables.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cerbrix_common::validate_host_name;
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;
use url::Url;

use crate::collectors::{DEFAULT_EXCLUDED_FS, Patterns};

/// Configuration file read when no path is given (and it exists).
pub const DEFAULT_CONFIG_PATH: &str = "/etc/cerbrix/agent.toml";

const MIN_SPOOL_BYTES: u64 = 1024 * 1024;

/// Errors raised while loading the configuration. Messages never contain secret values.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read configuration: {0}")]
    Read(String),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Base URL of the server; must be `https` unless `allow_insecure_http` is set.
    pub server_url: Url,
    pub host: String,
    /// Only needed until the agent has enrolled.
    pub enrollment_key: Option<String>,
    /// Extra PEM root certificate(s) to trust for the server's TLS certificate.
    pub ca_file: Option<PathBuf>,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default = "default_state_dir")]
    pub state_dir: PathBuf,
    #[serde(default = "default_spool_max_bytes")]
    pub spool_max_bytes: u64,
    #[serde(default)]
    pub intervals: Intervals,
    #[serde(default)]
    pub disk: DiskOptions,
    #[serde(default)]
    pub net: NetOptions,
}

/// How often each collector runs. Written as `30s`, `5m`, `1h`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Intervals {
    #[serde(with = "humantime_serde")]
    pub cpu: Duration,
    #[serde(with = "humantime_serde")]
    pub load: Duration,
    #[serde(with = "humantime_serde")]
    pub memory: Duration,
    #[serde(with = "humantime_serde")]
    pub uptime: Duration,
    #[serde(with = "humantime_serde")]
    pub disk: Duration,
    #[serde(with = "humantime_serde")]
    pub net: Duration,
}

impl Default for Intervals {
    fn default() -> Self {
        Self {
            cpu: Duration::from_secs(30),
            load: Duration::from_secs(30),
            memory: Duration::from_secs(30),
            uptime: Duration::from_secs(300),
            disk: Duration::from_secs(300),
            net: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiskOptions {
    /// File-system types that are not monitored.
    pub exclude_fs: Vec<String>,
    /// Mount point globs that are not monitored.
    pub exclude_mounts: Vec<String>,
}

impl Default for DiskOptions {
    fn default() -> Self {
        Self {
            exclude_fs: DEFAULT_EXCLUDED_FS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            exclude_mounts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetOptions {
    /// Interface name globs that are not monitored.
    pub exclude_interfaces: Vec<String>,
}

fn default_state_dir() -> PathBuf {
    PathBuf::from("/var/lib/cerbrix-agent")
}

fn default_spool_max_bytes() -> u64 {
    64 * 1024 * 1024
}

// Manual Debug: the enrolment key must never reach logs.
impl fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentConfig")
            .field("server_url", &self.server_url.as_str())
            .field("host", &self.host)
            .field(
                "enrollment_key",
                &self.enrollment_key.as_ref().map(|_| "<redacted>"),
            )
            .field("ca_file", &self.ca_file)
            .field("allow_insecure_http", &self.allow_insecure_http)
            .field("state_dir", &self.state_dir)
            .field("spool_max_bytes", &self.spool_max_bytes)
            .field("intervals", &self.intervals)
            .field("disk", &self.disk)
            .field("net", &self.net)
            .finish()
    }
}

impl AgentConfig {
    /// Loads the configuration. Later sources win: defaults, the TOML file, then
    /// environment variables such as `CERBRIX_AGENT_ENROLLMENT_KEY` (`__` separates
    /// nested keys, e.g. `CERBRIX_AGENT_INTERVALS__CPU`).
    ///
    /// An explicit `path` must exist; without one, [`DEFAULT_CONFIG_PATH`] is read
    /// if it exists.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut figment = Figment::new();
        match path {
            Some(path) => {
                std::fs::metadata(path)
                    .map_err(|e| ConfigError::Read(format!("{}: {e}", path.display())))?;
                figment = figment.merge(Toml::file(path));
            }
            None => {
                let default = Path::new(DEFAULT_CONFIG_PATH);
                if default.exists() {
                    figment = figment.merge(Toml::file(default));
                }
            }
        }
        figment = figment.merge(Env::prefixed("CERBRIX_AGENT_").split("__"));

        let config: AgentConfig = figment.extract().map_err(|e| {
            // A mistyped enrolment key would otherwise be echoed back in the message.
            let touches_key = e
                .clone()
                .into_iter()
                .any(|e| e.path.last().is_some_and(|s| s == "enrollment_key"));
            if touches_key {
                ConfigError::Invalid("enrollment_key is not a valid string".into())
            } else {
                ConfigError::Invalid(e.to_string())
            }
        })?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |msg: &str| Err(ConfigError::Invalid(msg.to_owned()));
        match self.server_url.scheme() {
            "https" => {}
            "http" if self.allow_insecure_http => {}
            "http" => {
                return invalid(
                    "server_url must use https; set allow_insecure_http = true to allow plain http",
                );
            }
            _ => return invalid("server_url must be an https URL"),
        }
        if self.server_url.host_str().is_none() {
            return invalid("server_url has no host");
        }
        if validate_host_name(&self.host).is_err() {
            return invalid(
                "host is not a valid host name: use 1 to 253 letters, digits, '.', '_' or '-', \
                 starting with a letter or digit",
            );
        }
        if self.enrollment_key.as_deref() == Some("") {
            return invalid("enrollment_key is empty; remove it or set a key");
        }
        if self.spool_max_bytes < MIN_SPOOL_BYTES {
            return invalid("spool_max_bytes must be at least 1048576 (1 MiB)");
        }
        let i = &self.intervals;
        for (name, value) in [
            ("cpu", i.cpu),
            ("load", i.load),
            ("memory", i.memory),
            ("uptime", i.uptime),
            ("disk", i.disk),
            ("net", i.net),
        ] {
            if value.is_zero() {
                return Err(ConfigError::Invalid(format!(
                    "intervals.{name} must be greater than zero"
                )));
            }
        }
        Patterns::new(&self.disk.exclude_mounts)
            .map_err(|e| ConfigError::Invalid(format!("disk.exclude_mounts is not valid: {e}")))?;
        Patterns::new(&self.net.exclude_interfaces).map_err(|e| {
            ConfigError::Invalid(format!("net.exclude_interfaces is not valid: {e}"))
        })?;
        Ok(())
    }
}

#[cfg(test)]
impl AgentConfig {
    /// A valid configuration for tests of other modules.
    pub(crate) fn for_tests(server_url: &str, state_dir: &Path) -> Self {
        Self {
            server_url: server_url.parse().unwrap(),
            host: "web-01".to_owned(),
            enrollment_key: Some("test-enrollment-key".to_owned()),
            ca_file: None,
            allow_insecure_http: true,
            state_dir: state_dir.to_path_buf(),
            spool_max_bytes: default_spool_max_bytes(),
            intervals: Intervals::default(),
            disk: DiskOptions::default(),
            net: NetOptions::default(),
        }
    }
}

#[cfg(test)]
// `figment::Jail` closures must return `figment::Error`, which is large.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;
    use figment::Jail;

    const MINIMAL: &str = "server_url = \"https://cerbrix.example.com\"\nhost = \"web-01\"\n";

    fn load_str(jail: &mut Jail, toml: &str) -> Result<AgentConfig, ConfigError> {
        jail.create_file("agent.toml", toml).unwrap();
        AgentConfig::load(Some(Path::new("agent.toml")))
    }

    fn invalid(result: Result<AgentConfig, ConfigError>) -> String {
        match result {
            Err(ConfigError::Invalid(msg)) => msg,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn config_rejects_http_url_without_insecure_flag() {
        Jail::expect_with(|jail| {
            let toml = MINIMAL.replace("https", "http");
            let msg = invalid(load_str(jail, &toml));
            assert!(
                msg.contains("server_url") && msg.contains("allow_insecure_http"),
                "{msg}"
            );
            let with_flag = format!("{toml}allow_insecure_http = true\n");
            assert!(load_str(jail, &with_flag).is_ok());
            Ok(())
        });
    }

    #[test]
    fn config_defaults_apply() {
        Jail::expect_with(|jail| {
            let cfg = load_str(jail, MINIMAL).unwrap();
            assert_eq!(cfg.state_dir, PathBuf::from("/var/lib/cerbrix-agent"));
            assert_eq!(cfg.spool_max_bytes, 64 * 1024 * 1024);
            assert!(!cfg.allow_insecure_http);
            assert!(cfg.enrollment_key.is_none() && cfg.ca_file.is_none());
            assert_eq!(cfg.intervals.cpu, Duration::from_secs(30));
            assert_eq!(cfg.intervals.load, Duration::from_secs(30));
            assert_eq!(cfg.intervals.memory, Duration::from_secs(30));
            assert_eq!(cfg.intervals.uptime, Duration::from_secs(300));
            assert_eq!(cfg.intervals.disk, Duration::from_secs(300));
            assert_eq!(cfg.intervals.net, Duration::from_secs(30));
            assert_eq!(cfg.disk.exclude_fs.len(), DEFAULT_EXCLUDED_FS.len());
            assert!(cfg.disk.exclude_mounts.is_empty() && cfg.net.exclude_interfaces.is_empty());
            Ok(())
        });
    }

    #[test]
    fn durations_parse_from_strings_and_env_overrides_file() {
        Jail::expect_with(|jail| {
            jail.set_env("CERBRIX_AGENT_HOST", "from-env");
            jail.set_env("CERBRIX_AGENT_INTERVALS__NET", "2m");
            let toml = format!("{MINIMAL}[intervals]\ncpu = \"5s\"\nuptime = \"1h\"\n");
            let cfg = load_str(jail, &toml).unwrap();
            assert_eq!(cfg.host, "from-env");
            assert_eq!(cfg.intervals.cpu, Duration::from_secs(5));
            assert_eq!(cfg.intervals.uptime, Duration::from_secs(3600));
            assert_eq!(cfg.intervals.net, Duration::from_secs(120));
            Ok(())
        });
    }

    #[test]
    fn rejects_bad_values() {
        Jail::expect_with(|jail| {
            let zero = format!("{MINIMAL}[intervals]\ncpu = \"0s\"\n");
            assert!(invalid(load_str(jail, &zero)).contains("intervals.cpu"));
            let small = format!("{MINIMAL}spool_max_bytes = 1000\n");
            assert!(invalid(load_str(jail, &small)).contains("spool_max_bytes"));
            let host = MINIMAL.replace("web-01", "-bad host");
            assert!(invalid(load_str(jail, &host)).contains("host"));
            let mounts = format!("{MINIMAL}[disk]\nexclude_mounts = [\"\"]\n");
            assert!(invalid(load_str(jail, &mounts)).contains("disk.exclude_mounts"));
            let ifaces = format!("{MINIMAL}[net]\nexclude_interfaces = [\"\"]\n");
            assert!(invalid(load_str(jail, &ifaces)).contains("net.exclude_interfaces"));
            let unknown = format!("{MINIMAL}unknown_option = 1\n");
            assert!(invalid(load_str(jail, &unknown)).contains("unknown_option"));
            Ok(())
        });
    }

    #[test]
    fn missing_explicit_file_is_a_read_error() {
        let err = AgentConfig::load(Some(Path::new("/definitely/not/here.toml"))).unwrap_err();
        assert!(matches!(err, ConfigError::Read(_)));
    }

    #[test]
    fn debug_and_errors_hide_the_enrollment_key() {
        Jail::expect_with(|jail| {
            let ok = format!("{MINIMAL}enrollment_key = \"s3cret-key\"\n");
            let shown = format!("{:?}", load_str(jail, &ok).unwrap());
            assert!(
                !shown.contains("s3cret-key") && shown.contains("<redacted>"),
                "{shown}"
            );
            let bad = format!("{MINIMAL}enrollment_key = 123456789\n");
            assert!(!invalid(load_str(jail, &bad)).contains("123456789"));
            Ok(())
        });
    }
}
