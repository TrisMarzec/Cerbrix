//! The committed JSON Schemas match the Rust types, and the example
//! configuration repository validates against them.

use std::fs;
use std::path::{Path, PathBuf};

use cerbrix_common::config::schema_json;
use cerbrix_common::{DashboardFile, DisplayFile, HostFile, NotificationFile, TemplateFile};
use schemars::JsonSchema;
use serde_json::{Value, json};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn schema_path(name: &str) -> PathBuf {
    repo_root()
        .join("schemas")
        .join(format!("{name}.schema.json"))
}

fn check<T: JsonSchema>(name: &str) {
    let expected = schema_json::<T>();
    let path = schema_path(name);
    if std::env::var_os("UPDATE_SCHEMAS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &expected).unwrap();
        return;
    }
    let actual = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        actual == expected,
        "{} is out of date; run `UPDATE_SCHEMAS=1 cargo test -p cerbrix-common --test schemas`",
        path.display()
    );
}

#[test]
fn committed_schemas_are_current() {
    check::<HostFile>("host");
    check::<TemplateFile>("template");
    check::<NotificationFile>("notification");
    check::<DashboardFile>("dashboard");
    check::<DisplayFile>("display");
}

fn validator(name: &str) -> jsonschema::Validator {
    let text = fs::read_to_string(schema_path(name)).unwrap();
    jsonschema::validator_for(&serde_json::from_str::<Value>(&text).unwrap()).unwrap()
}

fn json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            json_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

#[test]
fn example_repo_validates_against_schemas() {
    let root = repo_root().join("examples/config-repo");
    let mut checked = 0;
    for (dir, schema) in [
        ("hosts", "host"),
        ("templates", "template"),
        ("notifications", "notification"),
        ("dashboards", "dashboard"),
        ("displays", "display"),
    ] {
        let v = validator(schema);
        let mut files = Vec::new();
        json_files(&root.join(dir), &mut files);
        assert!(!files.is_empty(), "no example files in {dir}/");
        for f in files {
            let value: Value = serde_json::from_str(&fs::read_to_string(&f).unwrap()).unwrap();
            let errors: Vec<String> = v.iter_errors(&value).map(|e| e.to_string()).collect();
            assert!(errors.is_empty(), "{}: {errors:?}", f.display());
            checked += 1;
        }
    }
    assert_eq!(checked, 6);
}

#[test]
fn schema_rejects_what_serde_rejects() {
    let host = validator("host");
    assert!(!host.is_valid(&json!({"host": "a", "itmes": []})));
    assert!(!host.is_valid(&json!({
        "host": "a",
        "triggers": [{"name": "t", "expression": "no data for 3m", "severity": "error"}]
    })));
    let dashboard = validator("dashboard");
    assert!(!dashboard.is_valid(&json!({
        "name": "d",
        "hosts": {},
        "panels": [{"type": "pie", "x": 0, "y": 0, "w": 1, "h": 1}]
    })));
}
