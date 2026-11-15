//! Admin commands: host, enrolment key and unknown-agent management.

use std::fmt::Write as _;

use anyhow::{Context as _, anyhow, bail};
use cerbrix_common::validate_host_name;
use clap::Subcommand;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::gitops::format_issue;
use crate::state::{StateStore, SyncChanges, SyncStatus};

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    /// Manage hosts.
    #[command(subcommand)]
    Host(HostCommand),
    /// Manage enrolment keys.
    #[command(subcommand)]
    Key(KeyCommand),
    /// Show the Git sync state.
    #[command(subcommand)]
    Sync(SyncCommand),
    /// List agents that tried to enrol under an unknown name.
    UnknownAgents,
}

#[derive(Debug, Subcommand)]
pub enum HostCommand {
    /// Register a host so its agent can enrol.
    Add { name: String },
    /// Remove the agent binding so the host can enrol again.
    Release { name: String },
    /// Show a host and its effective configuration.
    Show { name: String },
}

#[derive(Debug, Subcommand)]
pub enum SyncCommand {
    /// Show whether the installation is in sync with the Git repository.
    Status,
    /// List the applied revisions, newest first.
    History {
        /// Number of entries to show (1 to 1000).
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(i64).range(1..=1000))]
        limit: i64,
    },
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

/// Runs an admin command and returns the text to print. With `gitops_enabled`, hosts are
/// managed by Git and cannot be added here.
pub async fn run(
    store: &StateStore,
    command: AdminCommand,
    now: OffsetDateTime,
    gitops_enabled: bool,
) -> anyhow::Result<String> {
    match command {
        AdminCommand::Host(HostCommand::Add { name }) => {
            if gitops_enabled {
                bail!("hosts are managed by Git; add a file under hosts/ in the repository");
            }
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
        AdminCommand::Host(HostCommand::Show { name }) => {
            let Some(host) = store.host_details(&name).await? else {
                bail!("unknown host {name}");
            };
            let mut out = format!(
                "name: {}\nactive: {}\naddress: {}\ngroups: {}\nsource file: {}\n",
                host.name,
                if host.active { "yes" } else { "no" },
                host.address.as_deref().unwrap_or("-"),
                if host.groups.is_empty() {
                    "-".to_owned()
                } else {
                    host.groups.join(", ")
                },
                host.source_file.as_deref().unwrap_or("-"),
            );
            match host.config {
                Some(config) => out.push_str(&serde_json::to_string_pretty(&config)?),
                None => {
                    out.push_str("no configuration from Git; the agent uses the built-in defaults")
                }
            }
            Ok(out)
        }
        AdminCommand::Sync(SyncCommand::Status) => {
            let state = store.sync_state().await?;
            let label = match state.status {
                SyncStatus::Never => "never synced",
                SyncStatus::InSync => "in sync",
                SyncStatus::OutOfSync => "out of sync",
            };
            let time = |t: Option<OffsetDateTime>| -> anyhow::Result<String> {
                Ok(match t {
                    Some(t) => t.to_offset(time::UtcOffset::UTC).format(&Rfc3339)?,
                    None => "-".to_owned(),
                })
            };
            let mut out = format!(
                "{label}\nactive commit: {}\nlast successful sync: {}\nlast attempt: {}",
                state.active_commit.as_deref().unwrap_or("-"),
                time(state.last_success_at)?,
                time(state.last_attempt_at)?,
            );
            for issue in &state.issues {
                write!(out, "\n{}", format_issue(issue))?;
            }
            Ok(out)
        }
        AdminCommand::Sync(SyncCommand::History { limit }) => {
            let entries = store.sync_history(limit).await?;
            if entries.is_empty() {
                return Ok("no sync history".to_owned());
            }
            let lines = entries
                .iter()
                .map(|e| {
                    let commit: String = e.commit.chars().take(12).collect();
                    Ok(format!(
                        "{} {commit} {}",
                        e.applied_at
                            .to_offset(time::UtcOffset::UTC)
                            .format(&Rfc3339)?,
                        describe_changes(&e.changes)
                    ))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(lines.join("\n"))
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

/// Counts of one apply, e.g. `hosts +2 ~1 -0, dashboards +1`. Hosts always show all three
/// counts; the other kinds appear only when they changed.
fn describe_changes(changes: &SyncChanges) -> String {
    let hosts = &changes.hosts;
    let mut parts = vec![format!(
        "hosts +{} ~{} -{}",
        hosts.added.len(),
        hosts.changed.len(),
        hosts.removed.len()
    )];
    for (label, set) in [
        ("templates", &changes.templates),
        ("notifications", &changes.notifications),
        ("dashboards", &changes.dashboards),
        ("displays", &changes.displays),
    ] {
        if set.added.is_empty() && set.changed.is_empty() && set.removed.is_empty() {
            continue;
        }
        let mut counts = Vec::new();
        if !set.added.is_empty() {
            counts.push(format!("+{}", set.added.len()));
        }
        if !set.changed.is_empty() {
            counts.push(format!("~{}", set.changed.len()));
        }
        if !set.removed.is_empty() {
            counts.push(format!("-{}", set.removed.len()));
        }
        parts.push(format!("{label} {}", counts.join(" ")));
    }
    parts.join(", ")
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
        run(&store, add(), now(), false).await.unwrap();
        let err = run(&store, add(), now(), false).await.unwrap_err();
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
        run(&store, release("web-01"), now(), false).await.unwrap();
        assert!(store.host_for_credential(&cred).await.unwrap().is_none());
        let err = run(&store, release("nope"), now(), false)
            .await
            .unwrap_err();
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
            false,
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
        run(
            &store,
            AdminCommand::Key(KeyCommand::Revoke { id }),
            t0,
            false,
        )
        .await
        .unwrap();
        assert!(!store.enrollment_key_valid(&key, t0).await.unwrap());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_agents_lists_recorded_names(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        assert_eq!(
            run(&store, AdminCommand::UnknownAgents, now(), false)
                .await
                .unwrap(),
            "no unknown agents"
        );
        store.record_unknown_agent("ghost-a", now()).await.unwrap();
        store.record_unknown_agent("ghost-b", now()).await.unwrap();
        let out = run(&store, AdminCommand::UnknownAgents, now(), false)
            .await
            .unwrap();
        assert!(out.contains("ghost-a") && out.contains("ghost-b"), "{out}");
    }

    fn example_compiled() -> crate::gitops::Compiled {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        crate::gitops::compile(&crate::gitops::read_tree(&dir, "c1").unwrap()).unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_add_refused_with_gitops(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let err = run(
            &store,
            AdminCommand::Host(HostCommand::Add {
                name: "web-01".to_owned(),
            }),
            now(),
            true,
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "hosts are managed by Git; add a file under hosts/ in the repository"
        );
        assert!(store.host_by_name("web-01").await.unwrap().is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_show_prints_effective_configuration(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        store
            .apply_config("c1", &example_compiled(), now(), 50)
            .await
            .unwrap();
        let show = |name: &str| {
            AdminCommand::Host(HostCommand::Show {
                name: name.to_owned(),
            })
        };
        let out = run(&store, show("web-01"), now(), true).await.unwrap();
        assert!(out.contains("name: web-01"), "{out}");
        assert!(out.contains("active: yes"), "{out}");
        assert!(out.contains("address: 10.0.4.12"), "{out}");
        assert!(out.contains("groups: production, web"), "{out}");
        assert!(out.contains("source file: hosts/web-01.json"), "{out}");
        assert!(out.contains("\"origin\": \"template:linux-base\""), "{out}");

        let err = run(&store, show("nope"), now(), true).await.unwrap_err();
        assert_eq!(err.to_string(), "unknown host nope");

        store.create_host("manual-01").await.unwrap();
        let out = run(&store, show("manual-01"), now(), false).await.unwrap();
        assert!(
            out.contains("no configuration from Git; the agent uses the built-in defaults"),
            "{out}"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn sync_status_out_of_sync_lists_issue(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let status = || AdminCommand::Sync(SyncCommand::Status);
        let out = run(&store, status(), now(), true).await.unwrap();
        assert!(out.starts_with("never synced"), "{out}");

        let t = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        store
            .apply_config("abc123", &example_compiled(), t, 50)
            .await
            .unwrap();
        store
            .record_sync_failure(
                Some("def456"),
                &[crate::gitops::SyncIssue {
                    file: Some("hosts/web-01.json".into()),
                    line: Some(3),
                    column: Some(5),
                    reason: "unexpected token".into(),
                }],
                t + Duration::minutes(1),
            )
            .await
            .unwrap();
        let out = run(&store, status(), now(), true).await.unwrap();
        assert!(out.starts_with("out of sync"), "{out}");
        assert!(out.contains("active commit: abc123"), "{out}");
        assert!(
            out.contains("last successful sync: 2027-01-15T08:00:00Z"),
            "{out}"
        );
        assert!(out.contains("last attempt: 2027-01-15T08:01:00Z"), "{out}");
        assert!(
            out.contains("hosts/web-01.json:3:5: unexpected token"),
            "{out}"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn sync_history_formats_counts(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let history = |limit| AdminCommand::Sync(SyncCommand::History { limit });
        let out = run(&store, history(20), now(), true).await.unwrap();
        assert_eq!(out, "no sync history");
        let t = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        store
            .apply_config("0123456789abcdef0123", &example_compiled(), t, 50)
            .await
            .unwrap();
        let out = run(&store, history(20), now(), true).await.unwrap();
        assert_eq!(
            out,
            "2027-01-15T08:00:00Z 0123456789ab hosts +2 ~0 -0, templates +1, notifications +1, dashboards +1, displays +1"
        );
    }
}
