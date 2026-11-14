//! Runs the built binary: `check-config` needs no configuration file and no database.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn example_repo() -> PathBuf {
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

fn run(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cerbrix-server"))
        .arg("check-config")
        .arg(dir)
        .env_remove("CERBRIX_CONFIG")
        // Nothing listens here: the command must not try to connect.
        .env("CERBRIX_DATABASE__URL", "postgres://x:x@127.0.0.1:1/none")
        .output()
        .unwrap()
}

#[test]
fn valid_repository_succeeds_without_config_or_database() {
    let output = run(&example_repo());
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "configuration is valid: 2 hosts, 1 template, 1 notification, 1 dashboard, 1 display"
    );
}

#[test]
fn broken_host_file_fails_with_issue_lines() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&example_repo(), dir.path());
    std::fs::write(dir.path().join("hosts/web-01.json"), "{ not json").unwrap();
    let output = run(dir.path());
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("hosts/web-01.json:"), "{stderr}");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("configuration is valid"));
}
