//! Admin commands: host, enrolment key and unknown-agent management.

use std::fmt::Write as _;

use anyhow::{Context as _, anyhow, bail};
use cerbrix_common::validate_host_name;
use clap::Subcommand;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::state::StateStore;

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    /// Manage hosts.
    #[command(subcommand)]
    Host(HostCommand),
    /// Manage enrolment keys.
    #[command(subcommand)]
    Key(KeyCommand),
    /// List agents that tried to enrol under an unknown name.
    UnknownAgents,
}

#[derive(Debug, Subcommand)]
pub enum HostCommand {
    /// Register a host so its agent can enrol.
    Add { name: String },
    /// Remove the agent binding so the host can enrol again.
    Release { name: String },
}

#[derive(Debug, Subcommand)]
pub enum KeyCommand {
    /// Create an enrolment key; the key is printed once.
    Create {
        #[arg(long)]
        description: String,
        /// Lifetime such as 30d, 12h or 45m.
        #[arg(long, value_parser = parse_duration)]
        expires_in: Duration,
    },
    /// Revoke an enrolment key by id.
    Revoke { id: Uuid },
}

/// Longest accepted lifetime in seconds (about 31 years); keeps expiry dates in range.
const MAX_SECONDS: i64 = 1_000_000_000;

/// Parses `<positive integer><d|h|m>`.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let bad =
        || format!("invalid duration {s:?}: expected a number and a unit, e.g. 30d, 12h, 45m");
    let unit = s.chars().last().ok_or_else(bad)?;
    let digits = &s[..s.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let n: i64 = digits
        .parse()
        .map_err(|_| format!("invalid duration {s:?}: too large"))?;
    if n == 0 {
        return Err(format!("invalid duration {s:?}: must be greater than zero"));
    }
    let secs_per_unit = match unit {
        'd' => 86_400,
        'h' => 3_600,
        'm' => 60,
        _ => return Err(bad()),
    };
    n.checked_mul(secs_per_unit)
        .filter(|secs| *secs <= MAX_SECONDS)
        .map(Duration::seconds)
        .ok_or_else(|| format!("invalid duration {s:?}: too large"))
}

/// Runs an admin command and returns the text to print.
pub async fn run(
    store: &StateStore,
    command: AdminCommand,
    now: OffsetDateTime,
) -> anyhow::Result<String> {
    match command {
        AdminCommand::Host(HostCommand::Add { name }) => {
            validate_host_name(&name).map_err(|e| anyhow!("{e}"))?;
            let host = store
                .create_host(&name)
                .await
                .with_context(|| format!("cannot add host {name:?}"))?;
            Ok(format!("host {} added", host.name))
        }
        AdminCommand::Host(HostCommand::Release { name }) => {
            let Some(host) = store.host_by_name(&name).await? else {
                bail!("host {name:?} not found");
            };
            store.release_binding(host.id).await?;
            Ok(format!("host {} released", host.name))
        }
        AdminCommand::Key(KeyCommand::Create {
            description,
            expires_in,
        }) => {
            let expires_at = now
                .checked_add(expires_in)
                .ok_or_else(|| anyhow!("expiry is out of range"))?;
            let (id, key) = store
                .create_enrollment_key(&description, expires_at)
                .await?;
            Ok(format!(
                "id: {id}\nexpires: {}\nkey: {key}\nThe key is shown only once; store it now.",
                expires_at.format(&Rfc3339)?
            ))
        }
        AdminCommand::Key(KeyCommand::Revoke { id }) => {
            store
                .revoke_enrollment_key(id)
                .await
                .with_context(|| format!("cannot revoke key {id}"))?;
            Ok(format!("key {id} revoked"))
        }
        AdminCommand::UnknownAgents => {
            let agents = store.unknown_agents().await?;
            if agents.is_empty() {
                return Ok("no unknown agents".to_owned());
            }
            let mut out = String::from("host\tattempts\tfirst seen\tlast seen");
            for a in agents {
                write!(
                    out,
                    "\n{}\t{}\t{}\t{}",
                    a.host_name,
                    a.attempts,
                    a.first_seen.format(&Rfc3339)?,
                    a.last_seen.format(&Rfc3339)?
                )?;
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    #[test]
    fn duration_parsing() {
        assert_eq!(parse_duration("30d").unwrap(), Duration::days(30));
        assert_eq!(parse_duration("12h").unwrap(), Duration::hours(12));
        assert_eq!(parse_duration("45m").unwrap(), Duration::minutes(45));
        for bad in [
            "30",
            "d",
            "-1d",
            "0d",
            "",
            "1.5h",
            "5x",
            "99999999999999999999d",
            "99999d",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_add_twice_names_the_host(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let add = || {
            AdminCommand::Host(HostCommand::Add {
                name: "web-01".to_owned(),
            })
        };
        run(&store, add(), now()).await.unwrap();
        let err = run(&store, add(), now()).await.unwrap_err();
        assert!(format!("{err:#}").contains("web-01"), "{err:#}");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_release_unbinds_and_unknown_host_errors(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let host = store.create_host("web-01").await.unwrap();
        let cred = store.bind_credential(host.id).await.unwrap();
        let release = |name: &str| {
            AdminCommand::Host(HostCommand::Release {
                name: name.to_owned(),
            })
        };
        run(&store, release("web-01"), now()).await.unwrap();
        assert!(store.host_for_credential(&cred).await.unwrap().is_none());
        let err = run(&store, release("nope"), now()).await.unwrap_err();
        assert!(err.to_string().contains("nope"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn key_create_expires_in_30_days(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let t0 = now();
        let out = run(
            &store,
            AdminCommand::Key(KeyCommand::Create {
                description: "ci".to_owned(),
                expires_in: parse_duration("30d").unwrap(),
            }),
            t0,
        )
        .await
        .unwrap();
        let key = out
            .lines()
            .find_map(|l| l.strip_prefix("key: "))
            .unwrap()
            .to_owned();
        assert!(key.starts_with("cbx_ek_"));
        assert!(store.enrollment_key_valid(&key, t0).await.unwrap());
        assert!(
            !store
                .enrollment_key_valid(&key, t0 + Duration::days(31))
                .await
                .unwrap()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn key_revoke_works(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let t0 = now();
        let (id, key) = store
            .create_enrollment_key("x", t0 + Duration::hours(1))
            .await
            .unwrap();
        run(&store, AdminCommand::Key(KeyCommand::Revoke { id }), t0)
            .await
            .unwrap();
        assert!(!store.enrollment_key_valid(&key, t0).await.unwrap());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_agents_lists_recorded_names(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        assert_eq!(
            run(&store, AdminCommand::UnknownAgents, now())
                .await
                .unwrap(),
            "no unknown agents"
        );
        store.record_unknown_agent("ghost-a", now()).await.unwrap();
        store.record_unknown_agent("ghost-b", now()).await.unwrap();
        let out = run(&store, AdminCommand::UnknownAgents, now())
            .await
            .unwrap();
        assert!(out.contains("ghost-a") && out.contains("ghost-b"), "{out}");
    }
}
