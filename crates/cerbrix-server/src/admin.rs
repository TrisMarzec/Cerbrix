//! Admin commands: hosts, enrolment keys, unknown agents, problems and notifications.

use std::fmt::Write as _;

use anyhow::{Context as _, anyhow, bail};
use cerbrix_common::validate_host_name;
use clap::Subcommand;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::alerting::channel::{Channel, SmtpChannel};
use crate::alerting::message::{reason_text, rfc3339, severity_word};
use crate::alerting::{EventKind, Notification};
use crate::auth::{Role, generate_password, hash_password, validate_password, validate_user_name};
use crate::config::{Config, SmtpConfig};
use crate::gitops::{format_issue, replace_control, replace_control_by_space};
use crate::state::{Problem, ProblemFilter, StateError, StateStore, SyncChanges, SyncStatus};

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
    /// List, inspect, acknowledge and close problems.
    #[command(subcommand)]
    Problem(ProblemCommand),
    /// Check the e-mail setup and the notification queue.
    #[command(subcommand)]
    Notify(NotifyCommand),
    /// List agents that tried to enrol under an unknown name.
    UnknownAgents,
    /// Manage the users of the web UI.
    #[command(subcommand)]
    User(UserCommand),
}

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Create a user. Without --password-stdin a password is generated and printed once.
    Create {
        /// Lower case letters, digits, '.', '_' and '-'.
        name: String,
        #[arg(long)]
        role: Role,
        /// Read the password from the first line of standard input.
        #[arg(long)]
        password_stdin: bool,
    },
    /// List the users.
    List,
    /// Change the role of a user; ends the user's sessions.
    SetRole { name: String, role: Role },
    /// Disable a user; ends the user's sessions.
    Disable { name: String },
    /// Enable a disabled user.
    Enable { name: String },
    /// Set a new password; ends the user's sessions. Without --password-stdin a password is
    /// generated and printed once.
    ResetPassword {
        name: String,
        /// Read the password from the first line of standard input.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Delete a user.
    Delete { name: String },
}

impl UserCommand {
    /// True when the command takes the password from standard input.
    pub fn reads_password_stdin(&self) -> bool {
        matches!(
            self,
            UserCommand::Create {
                password_stdin: true,
                ..
            } | UserCommand::ResetPassword {
                password_stdin: true,
                ..
            }
        )
    }
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
pub enum ProblemCommand {
    /// List open problems, most severe first.
    List {
        /// Include resolved problems, newest first.
        #[arg(long)]
        all: bool,
        /// Number of problems to show (1 to 1000).
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(i64).range(1..=1000))]
        limit: i64,
    },
    /// Show every field of one problem.
    Show {
        /// The id or at least its first 4 hexadecimal characters.
        id: String,
    },
    /// Acknowledge a problem.
    Ack {
        /// The id or at least its first 4 hexadecimal characters.
        id: String,
        /// Why or how the problem is being handled.
        #[arg(long)]
        comment: String,
        /// Who acknowledges the problem.
        #[arg(long, default_value = "admin")]
        by: String,
    },
    /// Close an open problem by hand, for example one of a disk that was unmounted or a
    /// network interface that was removed: no value will ever arrive to end it. The end is
    /// mailed to those who were told about the start. If the condition still holds, the next
    /// value opens a new problem.
    Close {
        /// The id or at least its first 4 hexadecimal characters.
        id: String,
        /// Why the problem is being closed.
        #[arg(long)]
        comment: String,
        /// Who closes the problem.
        #[arg(long, default_value = "admin")]
        by: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum NotifyCommand {
    /// Send one test message through the configured SMTP server.
    Test { address: String },
    /// Show the state of the notification queue.
    Status,
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

/// Runs an admin command and returns the text to print. With `gitops.enabled`, hosts are
/// managed by Git and cannot be added here.
pub async fn run(
    store: &StateStore,
    command: AdminCommand,
    now: OffsetDateTime,
    config: &Config,
) -> anyhow::Result<String> {
    match command {
        AdminCommand::Host(HostCommand::Add { name }) => {
            if config.gitops.enabled {
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
        AdminCommand::Problem(ProblemCommand::List { all, limit }) => {
            let filter = if all {
                ProblemFilter::All
            } else {
                ProblemFilter::Open
            };
            let problems = store.problems(filter, limit).await?;
            if problems.is_empty() {
                return Ok(if all {
                    "no problems"
                } else {
                    "no open problems"
                }
                .to_owned());
            }
            Ok(problems
                .iter()
                .map(problem_line)
                .collect::<Vec<_>>()
                .join("\n"))
        }
        AdminCommand::Problem(ProblemCommand::Show { id }) => {
            let p = find_problem(store, &id).await?;
            Ok(problem_details(&p))
        }
        AdminCommand::Problem(ProblemCommand::Close { id, comment, by }) => {
            let (comment, by) = comment_and_name(&comment, &by)?;
            let p = find_problem(store, &id).await?;
            if p.resolved_at.is_some() {
                bail!("problem {} is already resolved", p.id);
            }
            match store
                .close_problem(p.id, &by, &comment, now, config.smtp.enabled())
                .await
            {
                Ok(p) => Ok(format!("problem {} closed", p.id)),
                Err(StateError::NotFound) => bail!("no problem with id {}", p.id),
                Err(StateError::Conflict(_)) => bail!("problem {} is already resolved", p.id),
                Err(e) => Err(e.into()),
            }
        }
        AdminCommand::Problem(ProblemCommand::Ack { id, comment, by }) => {
            let (comment, by) = comment_and_name(&comment, &by)?;
            let p = find_problem(store, &id).await?;
            match store.acknowledge_problem(p.id, &by, &comment, now).await {
                Ok(p) => Ok(format!("problem {} acknowledged", p.id)),
                Err(StateError::NotFound) => bail!("no problem with id {}", p.id),
                Err(e) => Err(e.into()),
            }
        }
        AdminCommand::Notify(NotifyCommand::Test { address }) => {
            notify_test(&config.smtp, &address).await
        }
        AdminCommand::Notify(NotifyCommand::Status) => {
            let s = store.outbox_summary(now).await?;
            let oldest = match s.oldest_pending {
                Some(o) => format!(
                    "{} to {}, {} attempts, last error: {}",
                    rfc3339(o.created_at),
                    replace_control(&o.recipient),
                    o.attempts,
                    o.last_error
                        .as_deref()
                        .map_or_else(|| "none".to_owned(), replace_control)
                ),
                None => "none".to_owned(),
            };
            Ok(format!(
                "pending: {}\nsent in the last 24 hours: {}\nfailed in the last 24 hours: {}\n\
                 oldest pending: {oldest}",
                s.pending, s.sent_24h, s.failed_24h
            ))
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
        // Without the password text of standard input; `main` supplies it through `run_user`.
        AdminCommand::User(command) => run_user(store, command, None, now).await,
    }
}

/// Strips one trailing `\n` or `\r\n` from a line read from standard input and nothing else.
pub fn password_from_line(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

/// Runs a `user` command. `stdin_password` is the first line of standard input (as read by
/// the binary) and is used when the command has `--password-stdin`. The generated password
/// is part of the returned text only for `create` and `reset-password` without the flag;
/// it is never logged.
pub async fn run_user(
    store: &StateStore,
    command: UserCommand,
    stdin_password: Option<String>,
    now: OffsetDateTime,
) -> anyhow::Result<String> {
    let find = async |name: &str| {
        store
            .user_by_name(name)
            .await?
            .ok_or_else(|| anyhow!("user {name:?} not found"))
    };
    // The chosen password, validated, or a generated one; the flag says which.
    let choose = |from_stdin: bool| -> anyhow::Result<(String, bool)> {
        if from_stdin {
            let password = stdin_password
                .clone()
                .ok_or_else(|| anyhow!("no password was read from standard input"))?;
            validate_password(&password).map_err(|e| anyhow!("{e}"))?;
            Ok((password, false))
        } else {
            Ok((generate_password(), true))
        }
    };
    match command {
        UserCommand::Create {
            name,
            role,
            password_stdin,
        } => {
            validate_user_name(&name).map_err(|e| anyhow!("{e}"))?;
            let (password, generated) = choose(password_stdin)?;
            let hash = hash_password(password.clone()).await?;
            let user = store
                .create_user(&name, role, &hash, now)
                .await
                .with_context(|| format!("cannot create user {name:?}"))?;
            let mut out = format!("user {} created with role {}", user.name, user.role);
            if generated {
                write!(out, "\npassword: {password}")?;
            }
            Ok(out)
        }
        UserCommand::List => {
            let users = store.users().await?;
            if users.is_empty() {
                return Ok("no users".to_owned());
            }
            let lines = users
                .iter()
                .map(|u| {
                    Ok(format!(
                        "{} {} {} {}",
                        u.name,
                        u.role,
                        if u.disabled { "disabled" } else { "enabled" },
                        u.created_at
                            .to_offset(time::UtcOffset::UTC)
                            .format(&Rfc3339)?
                    ))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(lines.join("\n"))
        }
        UserCommand::SetRole { name, role } => {
            let user = find(&name).await?;
            store
                .set_user_role(user.id, role)
                .await
                .with_context(|| format!("cannot change the role of user {name:?}"))?;
            Ok(format!("user {name} now has role {role}"))
        }
        UserCommand::Disable { name } => {
            let user = find(&name).await?;
            store
                .set_user_disabled(user.id, true)
                .await
                .with_context(|| format!("cannot disable user {name:?}"))?;
            Ok(format!("user {name} disabled"))
        }
        UserCommand::Enable { name } => {
            let user = find(&name).await?;
            store.set_user_disabled(user.id, false).await?;
            Ok(format!("user {name} enabled"))
        }
        UserCommand::ResetPassword {
            name,
            password_stdin,
        } => {
            let user = find(&name).await?;
            let (password, generated) = choose(password_stdin)?;
            let hash = hash_password(password.clone()).await?;
            store.set_user_password(user.id, &hash, now).await?;
            let mut out = format!("password of user {name} changed");
            if generated {
                write!(out, "\npassword: {password}")?;
            }
            Ok(out)
        }
        UserCommand::Delete { name } => {
            let user = find(&name).await?;
            store
                .delete_user(user.id)
                .await
                .with_context(|| format!("cannot delete user {name:?}"))?;
            Ok(format!("user {name} deleted"))
        }
    }
}

/// `--comment` and `--by` with control characters replaced and trimmed; neither may be empty.
fn comment_and_name(comment: &str, by: &str) -> anyhow::Result<(String, String)> {
    let comment = replace_control_by_space(comment).trim().to_owned();
    if comment.is_empty() {
        bail!("--comment must not be empty");
    }
    let by = replace_control_by_space(by).trim().to_owned();
    if by.is_empty() {
        bail!("--by must not be empty");
    }
    Ok((comment, by))
}

/// Resolves an id or id prefix (at least 4 hexadecimal characters, dashes optional) to
/// exactly one problem.
async fn find_problem(store: &StateStore, input: &str) -> anyhow::Result<Problem> {
    let prefix: String = input
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_ascii_lowercase();
    if prefix.chars().count() < 4 {
        bail!("give at least 4 characters of the id");
    }
    if !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("{} is not a problem id", replace_control(input));
    }
    let mut found = store.problem_by_prefix(&prefix).await?;
    match found.len() {
        0 => bail!("no problem with id {}", replace_control(input)),
        1 => Ok(found.remove(0)),
        n => bail!("the id prefix matches {n} problems"),
    }
}

/// `<started>  <SEVERITY>  <host>  <trigger>[ (<instance>)]  <state>  <id prefix>`
fn problem_line(p: &Problem) -> String {
    let mut what = replace_control(&p.trigger);
    if !p.instance.is_empty() {
        what.push_str(&format!(" ({})", replace_control(&p.instance)));
    }
    let state = if let Some(end) = p.resolved_at {
        match &p.resolve_reason {
            Some(reason) => format!(
                "resolved {} ({})",
                rfc3339(end),
                replace_control(&reason_text(reason))
            ),
            None => format!("resolved {}", rfc3339(end)),
        }
    } else if let Some(who) = &p.acknowledged_by {
        format!("acknowledged by {}", replace_control(who))
    } else {
        "open".to_owned()
    };
    format!(
        "{}  {}  {}  {what}  {state}  {}",
        rfc3339(p.started_at),
        severity_word(p.severity),
        replace_control(&p.host),
        &p.id.to_string()[..8]
    )
}

/// One `Label: value` line per field.
fn problem_details(p: &Problem) -> String {
    let mut lines = vec![
        format!("Id: {}", p.id),
        format!("Host: {}", replace_control(&p.host)),
        format!("Trigger: {}", replace_control(&p.trigger)),
    ];
    if !p.instance.is_empty() {
        lines.push(format!("Instance: {}", replace_control(&p.instance)));
    }
    lines.push(format!("Severity: {}", severity_word(p.severity)));
    lines.push(format!("Detail: {}", replace_control(&p.summary)));
    lines.push(format!("Started: {}", rfc3339(p.started_at)));
    if let Some(end) = p.resolved_at {
        lines.push(format!("Ended: {}", rfc3339(end)));
        if let Some(reason) = &p.resolve_reason {
            lines.push(format!("Reason: {}", replace_control(&reason_text(reason))));
        }
    }
    if let Some(at) = p.acknowledged_at {
        lines.push(format!(
            "Acknowledged: {} by {}",
            rfc3339(at),
            replace_control(p.acknowledged_by.as_deref().unwrap_or("-"))
        ));
        if let Some(comment) = &p.ack_comment {
            lines.push(format!("Comment: {}", replace_control(comment)));
        }
    }
    lines.join("\n")
}

/// The message of `admin notify test`, clearly marked as a test.
fn test_notification() -> Notification {
    Notification {
        kind: EventKind::Started,
        host: "cerbrix-test".to_owned(),
        trigger: "Test message".to_owned(),
        instance: String::new(),
        severity: cerbrix_common::config::Severity::Info,
        summary: "This is a test message sent with \"cerbrix-server admin notify test\"."
            .to_owned(),
        started_at: OffsetDateTime::now_utc(),
        resolved_at: None,
        resolve_reason: None,
    }
}

/// Sends the test message. Needs no database.
pub async fn notify_test(smtp: &SmtpConfig, address: &str) -> anyhow::Result<String> {
    let Some(host) = smtp.host.as_deref() else {
        bail!("no SMTP server configured (smtp.host)");
    };
    let channel = SmtpChannel::new(smtp)?;
    channel.send(address, &test_notification()).await?;
    Ok(format!(
        "test message accepted by {}:{}",
        replace_control(host),
        smtp.port
    ))
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
    use cerbrix_common::config::Severity;

    use super::*;
    use crate::alerting::{Evaluation, EventKind, OpenProblem, ResolveProblem};
    use sqlx::PgPool;

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn cfg(gitops_enabled: bool) -> Config {
        use crate::config::{DatabaseConfig, MetricsConfig, ServerConfig};
        Config {
            server: ServerConfig {
                listen: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                tls_cert: None,
                tls_key: None,
                insecure_http: true,
            },
            database: DatabaseConfig {
                url: "postgres://localhost/unused".into(),
            },
            metrics: MetricsConfig {
                url: "http://localhost:8428".into(),
            },
            ingest: Default::default(),
            gitops: crate::config::GitopsConfig {
                enabled: gitops_enabled,
                ..Default::default()
            },
            alerting: Default::default(),
            smtp: Default::default(),
            auth: Default::default(),
        }
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
        run(&store, add(), now(), &cfg(false)).await.unwrap();
        let err = run(&store, add(), now(), &cfg(false)).await.unwrap_err();
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
        run(&store, release("web-01"), now(), &cfg(false))
            .await
            .unwrap();
        assert!(store.host_for_credential(&cred).await.unwrap().is_none());
        let err = run(&store, release("nope"), now(), &cfg(false))
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
            &cfg(false),
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
            &cfg(false),
        )
        .await
        .unwrap();
        assert!(!store.enrollment_key_valid(&key, t0).await.unwrap());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_agents_lists_recorded_names(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        assert_eq!(
            run(&store, AdminCommand::UnknownAgents, now(), &cfg(false))
                .await
                .unwrap(),
            "no unknown agents"
        );
        store.record_unknown_agent("ghost-a", now()).await.unwrap();
        store.record_unknown_agent("ghost-b", now()).await.unwrap();
        let out = run(&store, AdminCommand::UnknownAgents, now(), &cfg(false))
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
            &cfg(true),
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
            .apply_config("c1", &example_compiled(), now(), 50, false)
            .await
            .unwrap();
        let show = |name: &str| {
            AdminCommand::Host(HostCommand::Show {
                name: name.to_owned(),
            })
        };
        let out = run(&store, show("web-01"), now(), &cfg(true))
            .await
            .unwrap();
        assert!(out.contains("name: web-01"), "{out}");
        assert!(out.contains("active: yes"), "{out}");
        assert!(out.contains("address: 10.0.4.12"), "{out}");
        assert!(out.contains("groups: production, web"), "{out}");
        assert!(out.contains("source file: hosts/web-01.json"), "{out}");
        assert!(out.contains("\"origin\": \"template:linux-base\""), "{out}");

        let err = run(&store, show("nope"), now(), &cfg(true))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "unknown host nope");

        store.create_host("manual-01").await.unwrap();
        let out = run(&store, show("manual-01"), now(), &cfg(false))
            .await
            .unwrap();
        assert!(
            out.contains("no configuration from Git; the agent uses the built-in defaults"),
            "{out}"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn sync_status_out_of_sync_lists_issue(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let status = || AdminCommand::Sync(SyncCommand::Status);
        let out = run(&store, status(), now(), &cfg(true)).await.unwrap();
        assert!(out.starts_with("never synced"), "{out}");

        let t = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        store
            .apply_config("abc123", &example_compiled(), t, 50, false)
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
        let out = run(&store, status(), now(), &cfg(true)).await.unwrap();
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
        let out = run(&store, history(20), now(), &cfg(true)).await.unwrap();
        assert_eq!(out, "no sync history");
        let t = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        store
            .apply_config("0123456789abcdef0123", &example_compiled(), t, 50, false)
            .await
            .unwrap();
        let out = run(&store, history(20), now(), &cfg(true)).await.unwrap();
        assert_eq!(
            out,
            "2027-01-15T08:00:00Z 0123456789ab hosts +2 ~0 -0, templates +1, notifications +1, dashboards +1, displays +1"
        );
    }

    const T0_MS: i64 = 1_800_000_000_000;

    fn at(ms: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).unwrap()
    }

    fn open(trigger: &str, instance: &str, severity: Severity, at_ms: i64) -> OpenProblem {
        OpenProblem {
            trigger: trigger.to_owned(),
            instance: instance.to_owned(),
            severity,
            at_ms,
            summary: format!("{trigger} is bad"),
            kind: crate::alerting::ProblemKind::Compare,
        }
    }

    /// A store with the example configuration; returns the id of `web-01`.
    async fn alerting_store(pool: PgPool) -> (StateStore, Uuid) {
        let store = StateStore::from_pool(pool);
        store
            .apply_config("c1", &example_compiled(), at(T0_MS), 50, false)
            .await
            .unwrap();
        let host = store.host_by_name("web-01").await.unwrap().unwrap().id;
        (store, host)
    }

    async fn open_problems(
        store: &StateStore,
        host: Uuid,
        mail: bool,
        open: Vec<OpenProblem>,
    ) -> Vec<Uuid> {
        store
            .evaluate_host(host, mail, |_| Evaluation {
                open,
                ..Evaluation::default()
            })
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.problem_id)
            .collect()
    }

    fn list(all: bool) -> AdminCommand {
        AdminCommand::Problem(ProblemCommand::List { all, limit: 50 })
    }

    fn show(id: &str) -> AdminCommand {
        AdminCommand::Problem(ProblemCommand::Show { id: id.to_owned() })
    }

    fn ack(id: &str, comment: &str, by: Option<&str>) -> AdminCommand {
        AdminCommand::Problem(ProblemCommand::Ack {
            id: id.to_owned(),
            comment: comment.to_owned(),
            by: by.unwrap_or("admin").to_owned(),
        })
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn problem_list_formats_lines(pool: PgPool) {
        let (store, host) = alerting_store(pool).await;
        let c = cfg(false);
        assert_eq!(
            run(&store, list(false), now(), &c).await.unwrap(),
            "no open problems"
        );
        let ids = open_problems(
            &store,
            host,
            false,
            vec![open("Disk", "mount=/", Severity::Critical, T0_MS)],
        )
        .await;
        let out = run(&store, list(false), now(), &c).await.unwrap();
        assert_eq!(
            out,
            format!(
                "2027-01-15T08:00:00Z  CRITICAL  web-01  Disk (mount=/)  open  {}",
                &ids[0].to_string()[..8]
            )
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn problem_list_all_includes_resolved_with_reason(pool: PgPool) {
        let (store, host) = alerting_store(pool.clone()).await;
        let c = cfg(false);
        let ids = open_problems(
            &store,
            host,
            false,
            vec![
                open("Gone", "", Severity::Warning, T0_MS),
                open("Cpu", "", Severity::High, T0_MS + 1000),
                open("Acked", "", Severity::Info, T0_MS + 2000),
            ],
        )
        .await;
        sqlx::query(
            "UPDATE problems SET resolved_at = $2, resolve_reason = 'trigger_removed' \
             WHERE id = $1",
        )
        .bind(ids[0])
        .bind(at(T0_MS + 60_000))
        .execute(&pool)
        .await
        .unwrap();
        store
            .acknowledge_problem(ids[1], "anna", "on it", at(T0_MS + 5000))
            .await
            .unwrap();
        let open_only = run(&store, list(false), now(), &c).await.unwrap();
        assert!(!open_only.contains("Gone"), "{open_only}");
        let out = run(&store, list(true), now(), &c).await.unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out}");
        // Newest first.
        assert_eq!(
            lines[0],
            format!(
                "2027-01-15T08:00:02Z  INFO  web-01  Acked  open  {}",
                &ids[2].to_string()[..8]
            )
        );
        assert_eq!(
            lines[1],
            format!(
                "2027-01-15T08:00:01Z  HIGH  web-01  Cpu  acknowledged by anna  {}",
                &ids[1].to_string()[..8]
            )
        );
        assert_eq!(
            lines[2],
            format!(
                "2027-01-15T08:00:00Z  WARNING  web-01  Gone  resolved 2027-01-15T08:01:00Z \
                 (the trigger was removed from the configuration)  {}",
                &ids[0].to_string()[..8]
            )
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn problem_ack_by_prefix_and_default_name(pool: PgPool) {
        let (store, host) = alerting_store(pool).await;
        let c = cfg(false);
        let ids = open_problems(
            &store,
            host,
            false,
            vec![open("Disk", "", Severity::High, T0_MS)],
        )
        .await;
        let id = ids[0];
        let prefix = id.to_string()[..8].to_uppercase();
        let out = run(&store, ack(&prefix, "  looking\ninto it ", None), now(), &c)
            .await
            .unwrap();
        assert_eq!(out, format!("problem {id} acknowledged"));
        let p = &store.problems(ProblemFilter::All, 10).await.unwrap()[0];
        assert_eq!(p.acknowledged_by.as_deref(), Some("admin"));
        assert_eq!(p.ack_comment.as_deref(), Some("looking into it"));
        // Dashes are accepted, and a new acknowledgement replaces the earlier one.
        let dashed = id.to_string()[..13].to_owned();
        run(&store, ack(&dashed, "again", Some("a\x07b")), now(), &c)
            .await
            .unwrap();
        let p = &store.problems(ProblemFilter::All, 10).await.unwrap()[0];
        assert_eq!(p.acknowledged_by.as_deref(), Some("a b"));
        // The full id works too.
        run(&store, ack(&id.to_string(), "x", None), now(), &c)
            .await
            .unwrap();
        // An empty comment is refused.
        let err = run(&store, ack(&prefix, "  ", None), now(), &c)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "--comment must not be empty");
    }

    fn close(id: &str, comment: &str, by: Option<&str>) -> AdminCommand {
        AdminCommand::Problem(ProblemCommand::Close {
            id: id.to_owned(),
            comment: comment.to_owned(),
            by: by.unwrap_or("admin").to_owned(),
        })
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn problem_close_prints_closed_and_refuses_a_resolved_problem(pool: PgPool) {
        let (store, host) = alerting_store(pool.clone()).await;
        let mut c = cfg(false);
        let ids = open_problems(
            &store,
            host,
            true,
            vec![
                open("Disk", "mount=/media/usb", Severity::High, T0_MS),
                open("Net", "interface=veth1", Severity::High, T0_MS + 1000),
            ],
        )
        .await;
        let id = ids[0];
        let out = run(
            &store,
            close(&id.to_string()[..8], " unmounted\n", Some("anna")),
            at(T0_MS + 60_000),
            &c,
        )
        .await
        .unwrap();
        assert_eq!(out, format!("problem {id} closed"));
        let p = run(&store, show(&id.to_string()), now(), &c).await.unwrap();
        assert!(
            p.contains("Reason: the problem was closed by an operator\n"),
            "{p}"
        );
        assert!(p.contains("by anna\nComment: unmounted"), "{p}");
        let all = run(&store, list(true), now(), &c).await.unwrap();
        assert!(
            all.contains("resolved 2027-01-15T08:01:00Z (the problem was closed by an operator)"),
            "{all}"
        );
        // Without SMTP settings no end e-mail is queued.
        let ends = |pool: PgPool| async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notification_outbox WHERE event = 'resolved'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        };
        assert_eq!(ends(pool.clone()).await, 0);

        let err = run(&store, close(&id.to_string(), "again", None), now(), &c)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), format!("problem {id} is already resolved"));
        let err = run(&store, close("ffffffff", "x", None), now(), &c)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "no problem with id ffffffff");
        let err = run(&store, close(&id.to_string(), " ", None), now(), &c)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "--comment must not be empty");

        // With SMTP settings the end is mailed to those who were told about the start.
        c.smtp.host = Some("mail.example.com".to_owned());
        c.smtp.from = Some("cerbrix@example.com".to_owned());
        assert!(c.smtp.enabled());
        run(
            &store,
            close(&ids[1].to_string(), "removed", None),
            now(),
            &c,
        )
        .await
        .unwrap();
        assert_eq!(ends(pool).await, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn ambiguous_prefix_is_an_error(pool: PgPool) {
        let (store, host) = alerting_store(pool.clone()).await;
        let ids = open_problems(
            &store,
            host,
            false,
            vec![
                open("A", "", Severity::High, T0_MS),
                open("B", "", Severity::High, T0_MS),
            ],
        )
        .await;
        // Give both problems ids that share a prefix.
        for (n, id) in ids.iter().enumerate() {
            sqlx::query("UPDATE problems SET id = $2::uuid WHERE id = $1")
                .bind(id)
                .bind(format!("abcd0000-0000-0000-0000-00000000000{}", n + 1))
                .execute(&pool)
                .await
                .unwrap();
        }
        let c = cfg(false);
        let err = run(&store, show("abcd"), now(), &c).await.unwrap_err();
        assert_eq!(err.to_string(), "the id prefix matches 2 problems");
        let err = run(&store, ack("ABCD0000", "x", None), now(), &c)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "the id prefix matches 2 problems");
        let full = "abcd0000-0000-0000-0000-000000000002";
        assert!(run(&store, show(full), now(), &c).await.is_ok());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_id_is_an_error(pool: PgPool) {
        let (store, _) = alerting_store(pool).await;
        let c = cfg(false);
        let err = run(&store, show("ffff"), now(), &c).await.unwrap_err();
        assert_eq!(err.to_string(), "no problem with id ffff");
        let err = run(&store, ack("ffffffff", "x", None), now(), &c)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "no problem with id ffffffff");
        let err = run(&store, show("abc"), now(), &c).await.unwrap_err();
        assert_eq!(err.to_string(), "give at least 4 characters of the id");
        let err = run(&store, show("a-b-c"), now(), &c).await.unwrap_err();
        assert_eq!(err.to_string(), "give at least 4 characters of the id");
        assert!(run(&store, show("zzzz"), now(), &c).await.is_err());
        assert!(run(&store, show("%%%%"), now(), &c).await.is_err());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn problem_show_prints_every_field(pool: PgPool) {
        let (store, host) = alerting_store(pool).await;
        let c = cfg(false);
        let mut o = open("Disk", "mount=/", Severity::High, T0_MS);
        o.summary = "Disk\x1b[31m full".to_owned();
        let id = open_problems(&store, host, false, vec![o]).await[0];
        let out = run(&store, show(&id.to_string()[..6]), now(), &c)
            .await
            .unwrap();
        assert_eq!(
            out,
            format!(
                "Id: {id}\nHost: web-01\nTrigger: Disk\nInstance: mount=/\nSeverity: HIGH\n\
                 Detail: Disk?[31m full\nStarted: 2027-01-15T08:00:00Z"
            )
        );
        store
            .evaluate_host(host, false, |_| Evaluation {
                resolve: vec![ResolveProblem {
                    trigger: "Disk".into(),
                    instance: "mount=/".into(),
                    at_ms: T0_MS + 90_000,
                }],
                ..Evaluation::default()
            })
            .await
            .unwrap();
        store
            .acknowledge_problem(id, "anna", "swapped\tdisk", at(T0_MS + 5000))
            .await
            .unwrap();
        let out = run(&store, show(&id.to_string()), now(), &c).await.unwrap();
        assert_eq!(
            out,
            format!(
                "Id: {id}\nHost: web-01\nTrigger: Disk\nInstance: mount=/\nSeverity: HIGH\n\
                 Detail: Disk?[31m full\nStarted: 2027-01-15T08:00:00Z\n\
                 Ended: 2027-01-15T08:01:30Z\n\
                 Reason: the condition no longer holds\n\
                 Acknowledged: 2027-01-15T08:00:05Z by anna\nComment: swapped?disk"
            )
        );
    }

    #[test]
    fn data_with_control_characters_is_neutralised_in_the_list() {
        let p = Problem {
            id: Uuid::nil(),
            host_id: Uuid::nil(),
            host: "h\x1b]0;x".into(),
            trigger: "t\n".into(),
            instance: String::new(),
            severity: Severity::Info,
            summary: String::new(),
            started_at: at(T0_MS),
            resolved_at: None,
            resolve_reason: None,
            acknowledged_at: Some(at(T0_MS)),
            acknowledged_by: Some("w\rho".into()),
            ack_comment: None,
            kind: crate::alerting::ProblemKind::Compare,
        };
        let line = problem_line(&p);
        assert!(!line.contains(char::is_control), "{line:?}");
        assert!(line.contains("acknowledged by w?ho"), "{line}");
    }

    #[tokio::test]
    async fn notify_test_without_smtp_fails_with_hint() {
        let err = notify_test(&cfg(false).smtp, "ops@example.com")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "no SMTP server configured (smtp.host)");
    }

    #[test]
    fn test_notification_is_clearly_marked() {
        let n = test_notification();
        assert_eq!(n.kind, EventKind::Started);
        assert_eq!(n.subject(), "[Cerbrix] INFO cerbrix-test: Test message");
        assert!(n.body().contains("cerbrix-server admin notify test"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn notify_status_counts(pool: PgPool) {
        let (store, host) = alerting_store(pool).await;
        let c = cfg(false);
        let status = || AdminCommand::Notify(NotifyCommand::Status);
        let now = at(T0_MS + 2 * 3_600_000);
        assert_eq!(
            run(&store, status(), now, &c).await.unwrap(),
            "pending: 0\nsent in the last 24 hours: 0\nfailed in the last 24 hours: 0\n\
             oldest pending: none"
        );
        open_problems(
            &store,
            host,
            true,
            (0..4)
                .map(|i| open(&format!("T{i}"), "", Severity::High, T0_MS + i * 1000))
                .collect(),
        )
        .await;
        let items = store.claim_notifications(now, 10).await.unwrap();
        assert_eq!(items.len(), 4);
        let hour = Duration::hours(1);
        store
            .mark_notification_sent(items[0].id, now - hour)
            .await
            .unwrap();
        store
            .mark_notification_failed(items[1].id, "boom", now - hour)
            .await
            .unwrap();
        store
            .mark_notification_retry(items[2].id, "connection refused", now)
            .await
            .unwrap();
        store
            .mark_notification_sent(items[3].id, now - Duration::hours(48))
            .await
            .unwrap();
        assert_eq!(
            run(&store, status(), now, &c).await.unwrap(),
            "pending: 1\nsent in the last 24 hours: 1\nfailed in the last 24 hours: 1\n\
             oldest pending: 2027-01-15T08:00:02Z to ops@example.com, 1 attempts, \
             last error: connection refused"
        );
    }

    fn user_cmd(args: &[&str]) -> UserCommand {
        use clap::Parser as _;
        #[derive(Debug, clap::Parser)]
        struct Wrapper {
            #[command(subcommand)]
            command: UserCommand,
        }
        Wrapper::try_parse_from(std::iter::once("user").chain(args.iter().copied()))
            .unwrap()
            .command
    }

    async fn run_user_cmd(
        store: &StateStore,
        args: &[&str],
        password: Option<&str>,
    ) -> anyhow::Result<String> {
        run_user(store, user_cmd(args), password.map(str::to_owned), now()).await
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn user_create_prints_password_once(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let out = run_user_cmd(&store, &["create", "t.marzec", "--role", "admin"], None)
            .await
            .unwrap();
        let mut lines = out.lines();
        assert_eq!(lines.next(), Some("user t.marzec created with role admin"));
        let password = lines
            .next()
            .unwrap()
            .strip_prefix("password: ")
            .unwrap()
            .to_owned();
        assert_eq!(lines.next(), None);
        assert_eq!(password.chars().count(), 20);
        let user = store.user_by_name("t.marzec").await.unwrap().unwrap();
        let stored = store.user_password_hash(user.id).await.unwrap().unwrap();
        assert_ne!(stored, password);
        assert!(!stored.contains(&password));
        assert!(crate::auth::verify_password(password.clone(), stored).await);
        // The listing never shows it.
        let list = run_user_cmd(&store, &["list"], None).await.unwrap();
        assert!(!list.contains(&password));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn user_create_with_stdin_password_prints_no_password(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let secret = "an-entered-passphrase";
        let out = run_user_cmd(
            &store,
            &["create", "amy", "--role", "viewer", "--password-stdin"],
            Some(secret),
        )
        .await
        .unwrap();
        assert_eq!(out, "user amy created with role viewer");
        let user = store.user_by_name("amy").await.unwrap().unwrap();
        let stored = store.user_password_hash(user.id).await.unwrap().unwrap();
        assert!(crate::auth::verify_password(secret.to_owned(), stored).await);

        // A too short password is refused without echoing it, and creates nothing.
        let err = run_user_cmd(
            &store,
            &["create", "bob", "--role", "viewer", "--password-stdin"],
            Some("short"),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("at least 12"), "{err:#}");
        assert!(!format!("{err:#}").contains("short"), "{err:#}");
        assert!(store.user_by_name("bob").await.unwrap().is_none());

        // The flag without a password text (the binary supplies it) is an error.
        assert!(
            run_user_cmd(
                &store,
                &["create", "bob", "--role", "viewer", "--password-stdin"],
                None
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn password_line_from_stdin_text() {
        assert_eq!(password_from_line("secret\n"), "secret");
        assert_eq!(password_from_line("secret\r\n"), "secret");
        assert_eq!(password_from_line("secret"), "secret");
        // Only one line ending is removed; spaces and further line ends are part of it.
        assert_eq!(password_from_line(" secret \n"), " secret ");
        assert_eq!(password_from_line("secret\n\n"), "secret\n");
        assert_eq!(password_from_line("secret\r\r\n"), "secret\r");
        assert_eq!(password_from_line(""), "");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn user_create_rejects_a_bad_name_and_a_taken_name(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let err = run_user_cmd(&store, &["create", "Admin", "--role", "admin"], None)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("invalid user name"), "{err:#}");
        assert!(store.users().await.unwrap().is_empty());
        run_user_cmd(&store, &["create", "amy", "--role", "viewer"], None)
            .await
            .unwrap();
        let err = run_user_cmd(&store, &["create", "amy", "--role", "admin"], None)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("amy"), "{err:#}");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn user_list_format(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        assert_eq!(
            run_user_cmd(&store, &["list"], None).await.unwrap(),
            "no users"
        );
        let t = time::macros::datetime!(2027-01-15 08:00:00 UTC);
        store.create_user("zed", Role::Admin, "h", t).await.unwrap();
        let amy = store
            .create_user("amy", Role::Viewer, "h", t + Duration::hours(1))
            .await
            .unwrap();
        store.set_user_disabled(amy.id, true).await.unwrap();
        assert_eq!(
            run_user_cmd(&store, &["list"], None).await.unwrap(),
            "amy viewer disabled 2027-01-15T09:00:00Z\nzed admin enabled 2027-01-15T08:00:00Z"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn set_role_disable_delete_messages(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        run_user_cmd(&store, &["create", "root", "--role", "admin"], None)
            .await
            .unwrap();
        run_user_cmd(&store, &["create", "amy", "--role", "viewer"], None)
            .await
            .unwrap();
        let out = |args: &'static [&'static str]| run_user_cmd(&store, args, None);
        assert_eq!(
            out(&["set-role", "amy", "operator"]).await.unwrap(),
            "user amy now has role operator"
        );
        assert_eq!(out(&["disable", "amy"]).await.unwrap(), "user amy disabled");
        assert_eq!(out(&["enable", "amy"]).await.unwrap(), "user amy enabled");
        let reset = out(&["reset-password", "amy"]).await.unwrap();
        assert!(
            reset.starts_with("password of user amy changed\npassword: "),
            "{reset}"
        );
        assert_eq!(reset.lines().count(), 2);
        assert_eq!(out(&["delete", "amy"]).await.unwrap(), "user amy deleted");

        // The last enabled admin is protected, with a message that says why.
        for args in [
            &["set-role", "root", "viewer"][..],
            &["disable", "root"],
            &["delete", "root"],
        ] {
            let err = run_user_cmd(&store, args, None).await.unwrap_err();
            assert!(
                format!("{err:#}").contains("the last enabled admin cannot be changed"),
                "{err:#}"
            );
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn reset_password_with_stdin_replaces_the_hash(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        run_user_cmd(&store, &["create", "amy", "--role", "viewer"], None)
            .await
            .unwrap();
        let secret = "a-brand-new-passphrase";
        let out = run_user_cmd(
            &store,
            &["reset-password", "amy", "--password-stdin"],
            Some(secret),
        )
        .await
        .unwrap();
        assert_eq!(out, "password of user amy changed");
        let user = store.user_by_name("amy").await.unwrap().unwrap();
        let stored = store.user_password_hash(user.id).await.unwrap().unwrap();
        assert!(crate::auth::verify_password(secret.to_owned(), stored).await);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_user_is_an_error(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        for args in [
            &["set-role", "ghost", "viewer"][..],
            &["disable", "ghost"],
            &["enable", "ghost"],
            &["reset-password", "ghost"],
            &["delete", "ghost"],
        ] {
            let err = run_user_cmd(&store, args, None).await.unwrap_err();
            assert!(
                format!("{err:#}").contains("user \"ghost\" not found"),
                "{err:#}"
            );
        }
    }
}
