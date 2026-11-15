//! The Git source: fetches the configuration repository by running the `git`
//! program and hands the checkout to the tree reader.
//!
//! The URL is operator-supplied and the token and SSH key are secrets, so:
//! every argument is passed separately (never through a shell), git runs with a
//! cleared environment, the token travels in the process environment as a
//! config value (never on a command line), and everything that leaves this
//! module as text has passed through [`scrub`].

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use tokio::process::Command;

use super::tree::{ROOTS, SYMLINK_REASON, read_tree};
use super::{MAX_ISSUES, Snapshot, SyncIssue};
use crate::config::{GitUrlKind, GitopsConfig, classify_git_url, valid_branch};

/// Why a fetch did not produce a snapshot.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum SourceError {
    /// Git could not get the revision. The text is already scrubbed of secrets.
    #[error("cannot fetch the configuration repository: {0}")]
    Fetch(String),
    /// The revision was fetched but its contents are not acceptable.
    #[error("the configuration repository is invalid ({} issues)", .0.len())]
    Invalid(Vec<SyncIssue>),
}

/// Something that yields the current revision of the configuration.
#[async_trait]
pub trait ConfigSource: Send + Sync {
    async fn fetch(&self) -> Result<Snapshot, SourceError>;
}

/// The null device as git understands it on this platform.
pub(crate) const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// Variables copied from the server's environment. Nothing else is inherited, in
/// particular not `CERBRIX_GITOPS__TOKEN` or the database URL.
pub(crate) const ALLOWED_PASSTHROUGH: &[&str] = if cfg!(windows) {
    &["PATH", "SystemRoot", "TEMP", "TMP"]
} else {
    &["PATH"]
};

const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const TIMEOUT_REASON: &str = "git did not finish within 60 seconds";
const REASON_MAX_CHARS: usize = 300;
const SUBMODULE_REASON: &str = "submodules are not supported";

/// Fetches the configuration repository into a cache directory with `git`.
#[derive(Debug)]
pub struct GitSource {
    cfg: GitopsConfig,
    pub(crate) timeout: Duration,
}

impl GitSource {
    pub fn new(cfg: &GitopsConfig) -> Self {
        let mut cfg = cfg.clone();
        // The child runs in another directory, so a relative cache must be pinned.
        if let Ok(absolute) = std::path::absolute(&cfg.cache_dir) {
            cfg.cache_dir = absolute;
        }
        GitSource {
            cfg,
            timeout: GIT_TIMEOUT,
        }
    }
}

/// How a git call failed. `Failed` carries the scrubbed reason.
enum RunError {
    NotFound,
    Timeout,
    Failed(String),
}

impl From<RunError> for SourceError {
    fn from(error: RunError) -> Self {
        SourceError::Fetch(match error {
            RunError::NotFound => "the git program was not found".to_owned(),
            RunError::Timeout => TIMEOUT_REASON.to_owned(),
            RunError::Failed(reason) => reason,
        })
    }
}

/// Everything one fetch needs to run git.
struct Runner {
    env: Vec<(String, String)>,
    secrets: Vec<String>,
    timeout: Duration,
}

impl Runner {
    /// Runs `git args` in `dir` and returns its standard output.
    async fn run(&self, dir: &Path, args: &[&str]) -> Result<Vec<u8>, RunError> {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(dir)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Dropping the future on timeout drops the child, which kills it.
        let output = match tokio::time::timeout(self.timeout, command.output()).await {
            Err(_) => return Err(RunError::Timeout),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(RunError::NotFound);
            }
            Ok(Err(e)) => {
                return Err(RunError::Failed(
                    self.scrub(&format!("cannot run git: {e}")),
                ));
            }
            Ok(Ok(output)) => output,
        };
        if output.status.success() {
            return Ok(output.stdout);
        }
        let stderr = self.scrub(&String::from_utf8_lossy(&output.stderr));
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| line.chars().take(REASON_MAX_CHARS).collect::<String>())
            .unwrap_or_else(|| {
                format!(
                    "git {} failed with {}",
                    args.first().copied().unwrap_or(""),
                    output.status
                )
            });
        Err(RunError::Failed(reason))
    }

    fn scrub(&self, text: &str) -> String {
        let secrets: Vec<&str> = self.secrets.iter().map(String::as_str).collect();
        scrub(text, &secrets)
    }

    fn io_error(&self, what: &str, error: &std::io::Error) -> RunError {
        RunError::Failed(self.scrub(&format!("{what}: {error}")))
    }
}

#[async_trait]
impl ConfigSource for GitSource {
    async fn fetch(&self) -> Result<Snapshot, SourceError> {
        let cfg = &self.cfg;
        let url = cfg
            .url
            .as_deref()
            .ok_or_else(|| SourceError::Fetch("no repository URL is configured".into()))?;
        // The same rules as configuration validation: the URL never reaches git unchecked.
        classify_git_url(url)
            .map_err(|why| SourceError::Fetch(format!("the repository URL {why}")))?;
        if !valid_branch(&cfg.branch) {
            return Err(SourceError::Fetch(
                "the configured branch is not a valid branch name".into(),
            ));
        }

        create_cache_dir(&cfg.cache_dir)
            .map_err(|e| SourceError::Fetch(format!("cannot create the cache directory: {e}")))?;
        let repo = cfg.cache_dir.join("repo");
        let config = git_config(cfg);
        let mut secrets = Vec::new();
        if let Some(token) = &cfg.token {
            secrets.push(token.clone());
            secrets.push(base64_token(token));
        }
        let runner = Runner {
            env: git_env(&cfg.cache_dir, &config),
            secrets,
            timeout: self.timeout,
        };

        // The cache is disposable: if git fails or hangs (a killed git leaves lock
        // files behind), it is rebuilt and the whole sequence runs once more.
        let refspec = format!("refs/heads/{}", cfg.branch);
        match sync_once(&runner, &repo, url, &refspec).await {
            Err(SyncFail::Run(first @ (RunError::Failed(_) | RunError::Timeout))) => {
                // If the cache cannot be removed, the original failure is the useful one.
                if remove_repo(&runner, &repo).await.is_err() {
                    return Err(first.into());
                }
                sync_once(&runner, &repo, url, &refspec)
                    .await
                    .map_err(SourceError::from)
            }
            other => other.map_err(SourceError::from),
        }
    }
}

/// Why one pass failed: a git or io problem that a rebuild may fix, or a final answer.
enum SyncFail {
    Run(RunError),
    Final(SourceError),
}

impl From<RunError> for SyncFail {
    fn from(e: RunError) -> Self {
        SyncFail::Run(e)
    }
}

impl From<SourceError> for SyncFail {
    fn from(e: SourceError) -> Self {
        SyncFail::Final(e)
    }
}

impl From<SyncFail> for SourceError {
    fn from(e: SyncFail) -> Self {
        match e {
            SyncFail::Run(e) => e.into(),
            SyncFail::Final(e) => e,
        }
    }
}

async fn sync_once(
    runner: &Runner,
    repo: &Path,
    url: &str,
    refspec: &str,
) -> Result<Snapshot, SyncFail> {
    prepare(runner, repo, url).await?;
    runner
        .run(
            repo,
            &[
                "fetch",
                "--quiet",
                "--depth",
                "1",
                "--no-tags",
                "--",
                "origin",
                refspec,
            ],
        )
        .await?;
    runner
        .run(
            repo,
            &["checkout", "--quiet", "--force", "--detach", "FETCH_HEAD"],
        )
        .await?;
    runner.run(repo, &["clean", "--quiet", "-fdx"]).await?;

    let head = runner.run(repo, &["rev-parse", "HEAD"]).await?;
    let commit = String::from_utf8_lossy(&head).trim().to_owned();
    if !(commit.len() == 40 || commit.len() == 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(SourceError::Fetch("git reported an invalid commit id".into()).into());
    }

    // `core.symlinks=false` turns links into plain files on disk, so they are
    // looked for in the committed tree instead.
    let listing = runner.run(repo, &["ls-tree", "-r", "-z", "HEAD"]).await?;
    let mut issues = special_entries(&listing);

    tracing::debug!(commit = %commit, "fetched the configuration repository");
    let read = {
        let (repo, commit) = (repo.to_path_buf(), commit.clone());
        tokio::task::spawn_blocking(move || read_tree(&repo, &commit))
            .await
            .map_err(|_| SourceError::Fetch("reading the checkout failed".into()))?
    };
    match read {
        Ok(snapshot) if issues.is_empty() => Ok(snapshot),
        Ok(_) => Err(SourceError::Invalid(issues).into()),
        Err(more) => {
            issues.extend(more);
            issues.truncate(MAX_ISSUES);
            Err(SourceError::Invalid(issues).into())
        }
    }
}

/// Deletes the checkout. A killed git may still hold files for a moment (on
/// Windows), so a failure is retried a few times.
async fn remove_repo(runner: &Runner, repo: &Path) -> Result<(), RunError> {
    let mut attempt = 0;
    loop {
        match tokio::fs::remove_dir_all(repo).await {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if attempt >= 20 => return Err(runner.io_error("cannot reset the cache", &e)),
            Err(_) => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Symbolic links and submodules among the entries of `git ls-tree -r -z`, if
/// they are inside one of the configuration directories.
fn special_entries(listing: &[u8]) -> Vec<SyncIssue> {
    let mut issues = Vec::new();
    for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let entry = String::from_utf8_lossy(entry);
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let reason = match meta.split(' ').next() {
            Some("120000") => SYMLINK_REASON,
            Some("160000") => SUBMODULE_REASON,
            _ => continue,
        };
        if ROOTS.contains(&path.split('/').next().unwrap_or("")) {
            issues.push(SyncIssue::new(Some(path), reason));
        }
    }
    issues.truncate(MAX_ISSUES);
    issues
}

fn create_cache_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Makes sure `repo` is a git repository whose `origin` is `url`.
async fn prepare(runner: &Runner, repo: &Path, url: &str) -> Result<(), RunError> {
    if !repo.join(".git").exists() {
        match tokio::fs::remove_dir_all(repo).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(runner.io_error("cannot reset the cache", &e)),
        }
        tokio::fs::create_dir_all(repo)
            .await
            .map_err(|e| runner.io_error("cannot create the cache", &e))?;
        runner
            .run(
                repo,
                &["init", "--quiet", "--initial-branch=main", "--template="],
            )
            .await?;
        runner
            .run(repo, &["remote", "add", "origin", "--", url])
            .await?;
        return Ok(());
    }
    let current = runner.run(repo, &["remote", "get-url", "origin"]).await?;
    if String::from_utf8_lossy(&current).trim() != url {
        runner
            .run(repo, &["remote", "set-url", "origin", "--", url])
            .await?;
    }
    Ok(())
}

/// Base64 of `x-access-token:<token>`, the credential of the basic-auth header.
fn base64_token(token: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"))
}

/// A path as git and ssh want to read it: absolute where possible, `/` separators.
fn git_path(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let resolved = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    plain_path(&resolved)
}

fn plain_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.to_owned()
    }
}

/// Quotes `text` for the shell that runs `core.sshCommand`.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The local directory a local-path URL points to.
fn local_path(url: &str) -> PathBuf {
    if url
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("file://"))
    {
        if let Ok(path) = url::Url::parse(url)
            .map_err(|_| ())
            .and_then(|u| u.to_file_path())
        {
            return path;
        }
        return PathBuf::from(&url[7..]);
    }
    PathBuf::from(url)
}

/// The git configuration for one fetch, as key/value pairs. They reach git
/// through `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n`, so
/// the token is never on a command line.
pub(crate) fn git_config(cfg: &GitopsConfig) -> Vec<(String, String)> {
    let mut config: Vec<(String, String)> = Vec::new();
    let mut set = |key: &str, value: String| config.push((key.to_owned(), value));

    set("core.symlinks", "false".into());
    set("credential.helper", String::new());
    set("core.hooksPath", NULL_DEVICE.into());
    set("core.fsmonitor", "false".into());
    set("fetch.recurseSubmodules", "false".into());
    set("advice.detachedHead", "false".into());
    set("transfer.fsckObjects", "true".into());

    // Only the transports the source needs.
    let url = cfg.url.as_deref().unwrap_or_default();
    let kind = classify_git_url(url).ok();
    set("protocol.allow", "never".into());
    set("protocol.ext.allow", "never".into());
    set("protocol.https.allow", "always".into());
    set("protocol.ssh.allow", "always".into());
    let local = kind == Some(GitUrlKind::Local);
    set(
        "protocol.file.allow",
        if local { "always" } else { "never" }.into(),
    );

    // The checkout is owned by this process, but git also checks the owner of
    // a local origin.
    set("safe.directory", git_path(&cfg.cache_dir.join("repo")));
    if local {
        set("safe.directory", git_path(&local_path(url)));
    }

    if let Some(token) = &cfg.token {
        set(
            "http.extraHeader",
            format!("Authorization: Basic {}", base64_token(token)),
        );
    } else if let Some(key) = &cfg.ssh_key_file {
        let (hosts, mode) = match &cfg.known_hosts_file {
            Some(file) => (plain_path(file), "yes"),
            None => (git_path(&cfg.cache_dir.join("known_hosts")), "accept-new"),
        };
        set(
            "core.sshCommand",
            format!(
                "ssh -i {} -o IdentitiesOnly=yes -o BatchMode=yes -o UserKnownHostsFile={} \
                 -o StrictHostKeyChecking={mode}",
                sh_quote(&plain_path(key)),
                sh_quote(&hosts),
            ),
        );
    } else if matches!(kind, Some(GitUrlKind::Ssh | GitUrlKind::ScpLike)) {
        // No key: never let ssh ask questions.
        set("core.sshCommand", "ssh -o BatchMode=yes".into());
    }
    config
}

/// The complete environment git is started with.
pub(crate) fn git_env(cache_dir: &Path, config: &[(String, String)]) -> Vec<(String, String)> {
    let mut env = Vec::new();
    for name in ALLOWED_PASSTHROUGH {
        if let Some(value) = std::env::var_os(name) {
            env.push(((*name).to_owned(), value.to_string_lossy().into_owned()));
        }
    }
    let mut set = |k: &str, v: String| env.push((k.to_owned(), v));
    set("HOME", cache_dir.to_string_lossy().into_owned());
    set("GIT_TERMINAL_PROMPT", "0".into());
    set("GIT_CONFIG_NOSYSTEM", "1".into());
    set("GIT_CONFIG_GLOBAL", NULL_DEVICE.into());
    set("GIT_CONFIG_COUNT", config.len().to_string());
    for (i, (key, value)) in config.iter().enumerate() {
        set(&format!("GIT_CONFIG_KEY_{i}"), key.clone());
        set(&format!("GIT_CONFIG_VALUE_{i}"), value.clone());
    }
    env
}

/// Replaces every secret, and the user name and password of any URL, by `***`.
pub(crate) fn scrub(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_owned();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(secret, "***");
    }
    scrub_userinfo(&out)
}

fn scrub_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("://") {
        let (head, tail) = rest.split_at(pos + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c == '/' || c.is_whitespace() || matches!(c, '\'' | '"' | '<' | '>'))
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command as StdCommand;
    use std::time::Duration;

    fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo")
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// Runs git for the tests without the developer's global or system configuration.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = StdCommand::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
            .env("GIT_CONFIG_SYSTEM", NULL_DEVICE)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// A repository holding the example configuration, committed on `main`.
    fn origin() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(&fixture_dir(), dir.path());
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir
    }

    fn commit_all(dir: &Path, message: &str) -> String {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", message]);
        git(dir, &["rev-parse", "HEAD"])
    }

    fn cfg_for(url: &Path, cache: &Path) -> GitopsConfig {
        GitopsConfig {
            enabled: true,
            url: Some(url.to_str().unwrap().to_owned()),
            cache_dir: cache.to_path_buf(),
            ..GitopsConfig::default()
        }
    }

    fn source(url: &Path, cache: &Path) -> GitSource {
        GitSource::new(&cfg_for(url, cache))
    }

    fn fetch_error(result: Result<Snapshot, SourceError>) -> String {
        match result {
            Err(SourceError::Fetch(reason)) => reason,
            other => panic!("expected Fetch, got {other:?}"),
        }
    }

    #[test]
    fn scrub_removes_secrets() {
        let out = scrub(
            "fatal: unable to access 'https://x:tok123@host/r.git/'",
            &["tok123"],
        );
        assert!(!out.contains("tok123"), "{out}");
        assert!(out.contains("***"), "{out}");
        assert!(out.contains("host/r.git"), "{out}");
    }

    #[test]
    fn scrub_removes_base64_form_and_unknown_userinfo() {
        let b64 = base64_token("abc");
        let out = scrub(&format!("header Basic {b64} sent"), &["abc", &b64]);
        assert!(!out.contains(&b64), "{out}");
        let out = scrub("cloning https://alice:pw@host/r.git and ssh://bob@h/x", &[]);
        assert!(!out.contains("alice") && !out.contains("pw@"), "{out}");
        assert!(out.contains("host/r.git"), "{out}");
        assert_eq!(scrub("nothing here", &["", "tok"]), "nothing here");
    }

    #[tokio::test]
    async fn fetches_head_of_branch() {
        let origin = origin();
        let cache = tempfile::tempdir().unwrap();
        let snapshot = source(origin.path(), cache.path()).fetch().await.unwrap();
        assert_eq!(snapshot.commit, git(origin.path(), &["rev-parse", "HEAD"]));
        assert!(snapshot.files.contains_key("hosts/web-01.json"));
    }

    #[tokio::test]
    async fn second_fetch_sees_new_commit() {
        let origin = origin();
        let cache = tempfile::tempdir().unwrap();
        let src = source(origin.path(), cache.path());
        let first = src.fetch().await.unwrap();

        std::fs::write(
            origin.path().join("hosts/web-01.json"),
            b"{\"changed\":true}",
        )
        .unwrap();
        std::fs::remove_file(origin.path().join("dashboards/production.json")).unwrap();
        let head = commit_all(origin.path(), "change");

        let second = src.fetch().await.unwrap();
        assert_ne!(first.commit, second.commit);
        assert_eq!(second.commit, head);
        assert_eq!(second.files["hosts/web-01.json"], b"{\"changed\":true}");
        assert!(first.files.contains_key("dashboards/production.json"));
        assert!(!second.files.contains_key("dashboards/production.json"));
    }

    #[tokio::test]
    async fn unknown_branch_is_fetch_error() {
        let origin = origin();
        let cache = tempfile::tempdir().unwrap();
        let mut cfg = cfg_for(origin.path(), cache.path());
        cfg.branch = "nope".into();
        let reason = fetch_error(GitSource::new(&cfg).fetch().await);
        assert!(reason.contains("nope"), "{reason}");
    }

    #[tokio::test]
    async fn unreachable_repository_is_fetch_error() {
        let base = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let missing = base.path().join("does-not-exist");
        let reason = fetch_error(source(&missing, cache.path()).fetch().await);
        assert!(!reason.is_empty() && reason.len() <= 300, "{reason}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_in_repository_is_invalid() {
        let origin = origin();
        std::os::unix::fs::symlink("/etc/passwd", origin.path().join("hosts/evil.json")).unwrap();
        commit_all(origin.path(), "add symlink");
        let cache = tempfile::tempdir().unwrap();
        match source(origin.path(), cache.path()).fetch().await {
            Err(SourceError::Invalid(issues)) => assert!(
                issues
                    .iter()
                    .any(|i| i.file.as_deref() == Some("hosts/evil.json")),
                "{issues:?}"
            ),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submodule_entry_is_invalid() {
        let origin = origin();
        git(
            origin.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000,1111111111111111111111111111111111111111,hosts/sub",
            ],
        );
        git(origin.path(), &["commit", "-q", "-m", "gitlink"]);
        let cache = tempfile::tempdir().unwrap();
        match source(origin.path(), cache.path()).fetch().await {
            Err(SourceError::Invalid(issues)) => assert!(
                issues
                    .iter()
                    .any(|i| i.file.as_deref() == Some("hosts/sub")),
                "{issues:?}"
            ),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cache_survives_url_change() {
        let first = origin();
        let second = origin();
        std::fs::write(second.path().join("hosts/other.json"), b"{}").unwrap();
        commit_all(second.path(), "other");
        let cache = tempfile::tempdir().unwrap();

        let a = source(first.path(), cache.path()).fetch().await.unwrap();
        assert!(!a.files.contains_key("hosts/other.json"));
        let b = source(second.path(), cache.path()).fetch().await.unwrap();
        assert!(b.files.contains_key("hosts/other.json"));
        assert_eq!(b.commit, git(second.path(), &["rev-parse", "HEAD"]));
    }

    #[tokio::test]
    async fn corrupt_cache_is_rebuilt() {
        let origin = origin();
        let cache = tempfile::tempdir().unwrap();
        let src = source(origin.path(), cache.path());
        src.fetch().await.unwrap();
        std::fs::write(cache.path().join("repo/.git/HEAD"), b"garbage\n").unwrap();
        let snapshot = src.fetch().await.unwrap();
        assert_eq!(snapshot.commit, git(origin.path(), &["rev-parse", "HEAD"]));
    }

    async fn recovers_from_lock(lock: &str) {
        let origin = origin();
        let cache = tempfile::tempdir().unwrap();
        let src = source(origin.path(), cache.path());
        src.fetch().await.unwrap();
        let lock = cache.path().join("repo/.git").join(lock);
        std::fs::write(&lock, b"").unwrap();
        std::fs::write(origin.path().join("hosts/new.json"), b"{}").unwrap();
        let head = commit_all(origin.path(), "more");
        let snapshot = src.fetch().await.unwrap();
        assert_eq!(snapshot.commit, head);
        assert!(snapshot.files.contains_key("hosts/new.json"));
    }

    #[tokio::test]
    async fn stale_index_lock_is_recovered() {
        recovers_from_lock("index.lock").await;
    }

    #[tokio::test]
    async fn stale_shallow_lock_is_recovered() {
        recovers_from_lock("shallow.lock").await;
    }

    #[tokio::test]
    async fn cache_directory_is_created() {
        let origin = origin();
        let base = tempfile::tempdir().unwrap();
        let cache = base.path().join("a/b/cache");
        source(origin.path(), &cache).fetch().await.unwrap();
        assert!(cache.join("repo/.git").exists());
    }

    #[tokio::test]
    async fn file_url_is_a_local_path() {
        let origin = origin();
        let cache = tempfile::tempdir().unwrap();
        let mut cfg = cfg_for(origin.path(), cache.path());
        let path = origin.path().to_str().unwrap().replace('\\', "/");
        cfg.url = Some(if path.starts_with('/') {
            format!("file://{path}")
        } else {
            format!("file:///{path}")
        });
        let snapshot = GitSource::new(&cfg).fetch().await.unwrap();
        assert!(snapshot.files.contains_key("hosts/web-01.json"));
    }

    #[tokio::test]
    async fn timeout_kills_git_and_reports_it() {
        let origin = origin();
        let cache = tempfile::tempdir().unwrap();
        let mut src = source(origin.path(), cache.path());
        src.timeout = Duration::ZERO;
        let reason = fetch_error(src.fetch().await);
        assert_eq!(reason, "git did not finish within 60 seconds");
    }

    #[tokio::test]
    async fn fetch_errors_never_contain_the_token() {
        let cache = tempfile::tempdir().unwrap();
        let cfg = GitopsConfig {
            enabled: true,
            url: Some("https://127.0.0.1:1/org/r.git".into()),
            token: Some("tok-SECRET-123".into()),
            cache_dir: cache.path().to_path_buf(),
            ..GitopsConfig::default()
        };
        let reason = fetch_error(GitSource::new(&cfg).fetch().await);
        assert!(!reason.contains("tok-SECRET-123"), "{reason}");
        assert!(
            !reason.contains(&base64_token("tok-SECRET-123")),
            "{reason}"
        );
        let shown = format!("{:?}", GitSource::new(&cfg));
        assert!(!shown.contains("tok-SECRET-123"), "{shown}");
    }

    fn value<'a>(config: &'a [(String, String)], key: &str) -> Option<&'a str> {
        config
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn remote_cfg() -> GitopsConfig {
        GitopsConfig {
            enabled: true,
            url: Some("https://git.example.com/org/r.git".into()),
            cache_dir: PathBuf::from("/var/lib/cerbrix/git"),
            ..GitopsConfig::default()
        }
    }

    #[test]
    fn token_becomes_basic_auth_header() {
        let cfg = GitopsConfig {
            token: Some("abc".into()),
            ..remote_cfg()
        };
        let config = git_config(&cfg);
        assert_eq!(
            value(&config, "http.extraHeader"),
            Some(format!("Authorization: Basic {}", base64_token("abc")).as_str())
        );
        assert!(value(&config, "core.sshCommand").is_none());
    }

    #[test]
    fn ssh_key_without_known_hosts_trusts_on_first_use() {
        let cfg = GitopsConfig {
            url: Some("git@host:org/r.git".into()),
            ssh_key_file: Some(PathBuf::from("/keys/id_ed25519")),
            ..remote_cfg()
        };
        let config = git_config(&cfg);
        let ssh = value(&config, "core.sshCommand").expect("sshCommand");
        assert!(ssh.contains("-i '/keys/id_ed25519'"), "{ssh}");
        assert!(ssh.contains("IdentitiesOnly=yes"), "{ssh}");
        assert!(ssh.contains("BatchMode=yes"), "{ssh}");
        assert!(ssh.contains("StrictHostKeyChecking=accept-new"), "{ssh}");
        assert!(ssh.contains("known_hosts"), "{ssh}");
        assert!(ssh.contains("cerbrix"), "{ssh}");
        assert!(value(&config, "http.extraHeader").is_none());
    }

    #[test]
    fn ssh_key_with_known_hosts_is_strict() {
        let cfg = GitopsConfig {
            url: Some("ssh://git@host/org/r.git".into()),
            ssh_key_file: Some(PathBuf::from("/keys/id_ed25519")),
            known_hosts_file: Some(PathBuf::from("/etc/cerbrix/known hosts")),
            ..remote_cfg()
        };
        let config = git_config(&cfg);
        let ssh = value(&config, "core.sshCommand").expect("sshCommand");
        assert!(ssh.contains("StrictHostKeyChecking=yes"), "{ssh}");
        assert!(
            ssh.contains("UserKnownHostsFile='/etc/cerbrix/known hosts'"),
            "{ssh}"
        );
    }

    #[test]
    fn ssh_paths_with_quotes_are_quoted_safely() {
        let cfg = GitopsConfig {
            url: Some("git@host:r.git".into()),
            ssh_key_file: Some(PathBuf::from("/k/it's $(x)")),
            ..remote_cfg()
        };
        let config = git_config(&cfg);
        let ssh = value(&config, "core.sshCommand").unwrap();
        assert!(ssh.contains(r"-i '/k/it'\''s $(x)'"), "{ssh}");
    }

    #[test]
    fn always_disables_symlinks_and_helpers() {
        let token = GitopsConfig {
            token: Some("abc".into()),
            ..remote_cfg()
        };
        let key = GitopsConfig {
            url: Some("git@host:r.git".into()),
            ssh_key_file: Some(PathBuf::from("/k")),
            ..remote_cfg()
        };
        for cfg in [remote_cfg(), token, key] {
            let config = git_config(&cfg);
            assert_eq!(value(&config, "core.symlinks"), Some("false"));
            assert_eq!(value(&config, "credential.helper"), Some(""));
            assert_eq!(value(&config, "protocol.ext.allow"), Some("never"));
            assert_eq!(value(&config, "core.hooksPath"), Some(NULL_DEVICE));
            assert_eq!(value(&config, "core.fsmonitor"), Some("false"));
            assert_eq!(value(&config, "fetch.recurseSubmodules"), Some("false"));
            assert_eq!(value(&config, "advice.detachedHead"), Some("false"));
        }
    }

    #[test]
    fn local_protocol_and_safe_directory_only_for_local_urls() {
        let remote = git_config(&remote_cfg());
        assert_eq!(value(&remote, "protocol.file.allow"), Some("never"));
        let safe: Vec<_> = remote
            .iter()
            .filter(|(k, _)| k == "safe.directory")
            .collect();
        assert_eq!(safe.len(), 1, "only the cache repository: {safe:?}");

        let local = GitopsConfig {
            url: Some("file:///srv/origin".into()),
            ..remote_cfg()
        };
        let config = git_config(&local);
        assert_eq!(value(&config, "protocol.file.allow"), Some("always"));
        assert!(
            config
                .iter()
                .any(|(k, v)| k == "safe.directory" && v.ends_with("srv/origin")),
            "{config:?}"
        );
    }

    #[test]
    fn git_environment_is_minimal() {
        let env = git_env(
            Path::new("/var/lib/cerbrix/git"),
            &git_config(&remote_cfg()),
        );
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        for forbidden in [
            "CERBRIX_GITOPS__TOKEN",
            "CERBRIX_DATABASE__URL",
            "DATABASE_URL",
        ] {
            assert!(!names.contains(&forbidden), "{forbidden}");
        }
        assert!(names.contains(&"GIT_TERMINAL_PROMPT"));
        assert!(names.contains(&"GIT_CONFIG_NOSYSTEM"));
        assert!(names.contains(&"GIT_CONFIG_GLOBAL"));
        assert!(names.contains(&"GIT_CONFIG_COUNT"));
        assert!(names.contains(&"HOME"));
        assert!(
            names
                .iter()
                .all(|n| n.starts_with("GIT_") || ALLOWED_PASSTHROUGH.contains(n) || *n == "HOME"),
            "{names:?}"
        );
    }
}
