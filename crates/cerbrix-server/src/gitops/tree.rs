//! Reading a directory tree into a [`Snapshot`], and the `check-config` command.
//!
//! The reader is a security boundary: it only descends into the five
//! configuration roots, never follows symbolic links and never reads a file
//! that is not a regular `.json` file inside the given directory.

use std::fs;
use std::io::Read;
use std::path::Path;

use super::{MAX_ISSUES, Snapshot, SyncIssue, compile};

/// A `.json` file larger than this is rejected.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// A tree with more files than this is rejected.
pub const MAX_FILES: usize = 20_000;
/// A snapshot holds at most this many bytes of file content in total.
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// Directories nested deeper than this below a root are rejected.
const MAX_DEPTH: usize = 16;

const ROOTS: [&str; 5] = [
    "hosts",
    "templates",
    "notifications",
    "dashboards",
    "displays",
];

const SYMLINK_REASON: &str = "symbolic links are not allowed in the configuration repository";

/// Bounds on the size of a tree; injectable so tests do not need huge trees.
struct Limits {
    max_files: usize,
    max_total_bytes: u64,
}

const DEFAULT_LIMITS: Limits = Limits {
    max_files: MAX_FILES,
    max_total_bytes: MAX_TOTAL_BYTES,
};

struct Walk {
    limits: Limits,
    snapshot: Snapshot,
    issues: Vec<SyncIssue>,
    files_seen: usize,
    total_bytes: u64,
    /// Set when a tree-wide limit is hit; ends the walk.
    stopped: Option<String>,
}

impl Walk {
    fn full(&self) -> bool {
        self.issues.len() >= MAX_ISSUES
    }

    fn issue(&mut self, file: &str, reason: impl Into<String>) {
        if !self.full() {
            self.issues.push(SyncIssue::new(Some(file), reason));
        }
    }

    fn stop_total_bytes(&mut self) {
        self.stopped = Some(format!(
            "the repository's configuration files add up to more than {} MiB",
            self.limits.max_total_bytes / (1024 * 1024)
        ));
    }

    /// Stops the whole walk: a tree-wide limit was hit or there are too many issues.
    fn done(&self) -> bool {
        self.full() || self.stopped.is_some()
    }
}

/// Reads the five configuration directories below `root` into a snapshot.
///
/// A missing directory is not an error. Symbolic links, special files,
/// oversized `.json` files and more than [`MAX_FILES`] files are reported as
/// issues. Keys use `/` as separator on every platform.
pub fn read_tree(root: &Path, commit: &str) -> Result<Snapshot, Vec<SyncIssue>> {
    read_tree_with(root, commit, DEFAULT_LIMITS)
}

fn read_tree_with(root: &Path, commit: &str, limits: Limits) -> Result<Snapshot, Vec<SyncIssue>> {
    let mut walk = Walk {
        limits,
        snapshot: Snapshot {
            commit: commit.to_owned(),
            files: Default::default(),
        },
        issues: Vec::new(),
        files_seen: 0,
        total_bytes: 0,
        stopped: None,
    };

    for name in ROOTS {
        if walk.done() {
            break;
        }
        let path = root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => walk.issue(name, SYMLINK_REASON),
            Ok(meta) if meta.is_dir() => walk_dir(&mut walk, &path, name, 0),
            Ok(_) => walk.issue(name, "expected a directory"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => walk.issue(name, format!("cannot read: {err}")),
        }
    }

    if let Some(reason) = walk.stopped.take() {
        walk.issues.truncate(MAX_ISSUES - 1);
        walk.issues.push(SyncIssue::new(None, reason));
    }

    if walk.issues.is_empty() {
        Ok(walk.snapshot)
    } else {
        Err(walk.issues)
    }
}

fn walk_dir(walk: &mut Walk, dir: &Path, rel: &str, depth: usize) {
    if depth > MAX_DEPTH {
        walk.issue(rel, "directories are nested too deeply");
        return;
    }
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(err) => {
            walk.issue(rel, format!("cannot read directory: {err}"));
            return;
        }
    };
    let mut entries = Vec::new();
    for entry in read_dir {
        match entry {
            Ok(entry) => entries.push(entry),
            // A dropped entry could be a missing file in a snapshot judged valid.
            Err(err) => walk.issue(rel, format!("cannot read directory entry: {err}")),
        }
    }
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        if walk.done() {
            return;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            walk.issue(rel, "a file name is not valid UTF-8");
            continue;
        };
        let key = format!("{rel}/{name}");
        let path = entry.path();
        // `symlink_metadata`, never `metadata`: a link must not be followed.
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(err) => {
                walk.issue(&key, format!("cannot read: {err}"));
                continue;
            }
        };
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            walk.issue(&key, SYMLINK_REASON);
        } else if file_type.is_dir() {
            walk_dir(walk, &path, &key, depth + 1);
        } else if file_type.is_file() {
            walk.files_seen += 1;
            if walk.files_seen > walk.limits.max_files {
                walk.stopped = Some(format!(
                    "the repository has more than {} files",
                    walk.limits.max_files
                ));
                return;
            }
            if name.ends_with(".json") {
                read_file(walk, &path, &key, meta.len());
            }
        } else {
            walk.issue(&key, "not a regular file or directory");
        }
    }
}

fn read_file(walk: &mut Walk, path: &Path, key: &str, len: u64) {
    let too_large = format!("file is larger than {MAX_FILE_BYTES} bytes");
    if len > MAX_FILE_BYTES {
        walk.issue(key, too_large);
        return;
    }
    if walk.total_bytes + len > walk.limits.max_total_bytes {
        walk.stop_total_bytes();
        return;
    }
    // Read at most one byte more than allowed, in case the file grew since
    // the metadata call.
    let mut bytes = Vec::new();
    let read =
        fs::File::open(path).and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes));
    match read {
        Ok(_) if bytes.len() as u64 > MAX_FILE_BYTES => walk.issue(key, too_large),
        Ok(_) if walk.total_bytes + bytes.len() as u64 > walk.limits.max_total_bytes => {
            walk.stop_total_bytes();
        }
        Ok(_) => {
            walk.total_bytes += bytes.len() as u64;
            walk.snapshot.files.insert(key.to_owned(), bytes);
        }
        Err(err) => walk.issue(key, format!("cannot read: {err}")),
    }
}

/// Formats an issue as `file:line:column: reason`, `file: reason` or `reason`.
pub fn format_issue(issue: &SyncIssue) -> String {
    match (&issue.file, issue.line, issue.column) {
        (Some(file), Some(line), Some(column)) => {
            format!("{file}:{line}:{column}: {}", issue.reason)
        }
        (Some(file), _, _) => format!("{file}: {}", issue.reason),
        (None, _, _) => issue.reason.clone(),
    }
}

fn count(n: usize, singular: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {singular}s")
    }
}

/// Validates a checkout of the configuration repository.
///
/// Returns the summary line, or one formatted line per issue. Needs neither a
/// configuration file nor a database.
pub fn check_config(dir: &Path) -> Result<String, Vec<String>> {
    use super::DocumentKind::{Dashboard, Display, Notification, Template};

    let to_lines = |issues: Vec<SyncIssue>| issues.iter().map(format_issue).collect::<Vec<_>>();
    let snapshot = read_tree(dir, "").map_err(to_lines)?;
    let compiled = compile(&snapshot).map_err(to_lines)?;
    let documents = |kind| {
        compiled
            .documents
            .iter()
            .filter(|document| document.kind == kind)
            .count()
    };
    Ok(format!(
        "configuration is valid: {}, {}, {}, {}, {}",
        count(compiled.hosts.len(), "host"),
        count(documents(Template), "template"),
        count(documents(Notification), "notification"),
        count(documents(Dashboard), "dashboard"),
        count(documents(Display), "display"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, content: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn reads_nested_files_with_forward_slashes() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "hosts/db/db-01.json", b"{}");
        let snapshot = read_tree(dir.path(), "abc").unwrap();
        assert_eq!(snapshot.commit, "abc");
        assert_eq!(
            snapshot.files.keys().collect::<Vec<_>>(),
            ["hosts/db/db-01.json"]
        );
        assert_eq!(snapshot.files["hosts/db/db-01.json"], b"{}");
    }

    #[test]
    fn ignores_files_outside_roots_and_non_json() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "README.md", b"x");
        write(dir.path(), "other/a.json", b"{}");
        write(dir.path(), ".git/config.json", b"{}");
        write(dir.path(), "hosts/notes.txt", b"x");
        write(dir.path(), "hosts/a.json", b"{}");
        let snapshot = read_tree(dir.path(), "").unwrap();
        assert_eq!(snapshot.files.keys().collect::<Vec<_>>(), ["hosts/a.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("hosts")).unwrap();
        // The target does not exist: opening the link would give a read error.
        std::os::unix::fs::symlink(
            dir.path().join("does-not-exist"),
            dir.path().join("hosts/a.json"),
        )
        .unwrap();
        let issues = read_tree(dir.path(), "").unwrap_err();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].file.as_deref(), Some("hosts/a.json"));
        assert!(issues[0].reason.contains("symbolic links are not allowed"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("hosts")).unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("does-not-exist"),
            dir.path().join("hosts/sub"),
        )
        .unwrap();
        // A symbolic link in place of a root directory is rejected as well.
        std::os::unix::fs::symlink(
            dir.path().join("does-not-exist"),
            dir.path().join("templates"),
        )
        .unwrap();
        let issues = read_tree(dir.path(), "").unwrap_err();
        let files: Vec<_> = issues.iter().map(|i| i.file.as_deref()).collect();
        assert_eq!(files, [Some("hosts/sub"), Some("templates")]);
        assert!(
            issues
                .iter()
                .all(|i| i.reason.contains("symbolic links are not allowed"))
        );
    }

    #[test]
    fn rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "hosts/big.json",
            &vec![b' '; MAX_FILE_BYTES as usize + 1],
        );
        write(
            dir.path(),
            "hosts/ok.json",
            &vec![b' '; MAX_FILE_BYTES as usize],
        );
        let issues = read_tree(dir.path(), "").unwrap_err();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].file.as_deref(), Some("hosts/big.json"));
        assert!(issues[0].reason.contains("larger than"));
    }

    #[test]
    fn too_many_files_stops_the_walk_with_one_issue() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write(dir.path(), &format!("hosts/h{i}.json"), b"{}");
        }
        // After the limit, an oversized file would give a second issue if the walk went on.
        write(
            dir.path(),
            "hosts/z-big.json",
            &vec![b' '; MAX_FILE_BYTES as usize + 1],
        );
        let limits = Limits {
            max_files: 3,
            max_total_bytes: MAX_TOTAL_BYTES,
        };
        let issues = read_tree_with(dir.path(), "", limits).unwrap_err();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].file, None);
        assert!(issues[0].reason.contains("more than 3 files"));
    }

    #[test]
    fn too_many_bytes_stops_the_walk_with_one_issue() {
        let dir = tempfile::tempdir().unwrap();
        let mib = 1024 * 1024;
        write(dir.path(), "hosts/a.json", &vec![b' '; mib]);
        write(dir.path(), "hosts/b.json", &vec![b' '; mib]);
        write(dir.path(), "hosts/c.json", &vec![b' '; mib]);
        write(
            dir.path(),
            "hosts/z-big.json",
            &vec![b' '; MAX_FILE_BYTES as usize + 1],
        );
        let limits = Limits {
            max_files: MAX_FILES,
            max_total_bytes: 2 * mib as u64,
        };
        let issues = read_tree_with(dir.path(), "", limits).unwrap_err();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].file, None);
        assert!(issues[0].reason.contains("2 MiB"), "{}", issues[0].reason);
    }

    #[test]
    fn total_bytes_at_the_limit_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "hosts/a.json", &[b' '; 100]);
        write(dir.path(), "hosts/b.json", &[b' '; 100]);
        let limits = Limits {
            max_files: MAX_FILES,
            max_total_bytes: 200,
        };
        assert_eq!(
            read_tree_with(dir.path(), "", limits).unwrap().files.len(),
            2
        );
    }

    #[test]
    fn unreadable_root_directory_is_an_issue() {
        // A directory that cannot be listed is reported, not skipped.
        let dir = tempfile::tempdir().unwrap();
        let mut walk = Walk {
            limits: DEFAULT_LIMITS,
            snapshot: Snapshot::default(),
            issues: Vec::new(),
            files_seen: 0,
            total_bytes: 0,
            stopped: None,
        };
        walk_dir(&mut walk, &dir.path().join("gone"), "hosts/gone", 0);
        assert_eq!(walk.issues.len(), 1);
        assert_eq!(walk.issues[0].file.as_deref(), Some("hosts/gone"));
        assert!(walk.issues[0].reason.starts_with("cannot read directory"));
    }

    #[test]
    fn rejects_root_that_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "hosts", b"x");
        let issues = read_tree(dir.path(), "").unwrap_err();
        assert_eq!(issues[0].file.as_deref(), Some("hosts"));
        assert_eq!(issues[0].reason, "expected a directory");
    }

    #[test]
    fn missing_roots_give_empty_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = read_tree(dir.path(), "c1").unwrap();
        assert!(snapshot.files.is_empty());
        assert_eq!(snapshot.commit, "c1");
    }

    #[test]
    fn format_issue_variants() {
        let mut issue = SyncIssue::new(Some("hosts/a.json"), "expected `,`");
        issue.line = Some(3);
        issue.column = Some(7);
        assert_eq!(format_issue(&issue), "hosts/a.json:3:7: expected `,`");
        let issue = SyncIssue::new(Some("hosts/a.json"), "unknown template \"x\"");
        assert_eq!(format_issue(&issue), "hosts/a.json: unknown template \"x\"");
        let issue = SyncIssue::new(None, "repository unreachable");
        assert_eq!(format_issue(&issue), "repository unreachable");
    }

    #[test]
    fn counts_are_pluralised() {
        assert_eq!(count(0, "host"), "0 hosts");
        assert_eq!(count(1, "host"), "1 host");
        assert_eq!(count(2, "template"), "2 templates");
    }

    #[test]
    fn check_config_summarises_the_example_repository() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        assert_eq!(
            check_config(&dir).unwrap(),
            "configuration is valid: 2 hosts, 1 template, 1 notification, 1 dashboard, 1 display"
        );
    }

    #[test]
    fn check_config_reports_issue_lines() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "hosts/a.json", b"{");
        let lines = check_config(dir.path()).unwrap_err();
        assert!(lines[0].starts_with("hosts/a.json:"), "{lines:?}");
    }
}
