use std::collections::BTreeMap;
use std::path::Path;

use cerbrix_common::Severity;

use super::*;

fn snap(files: &[(&str, &str)]) -> Snapshot {
    Snapshot {
        commit: "c0ffee".into(),
        files: files
            .iter()
            .map(|(p, c)| ((*p).to_owned(), c.as_bytes().to_vec()))
            .collect(),
    }
}

fn issues_of(files: &[(&str, &str)]) -> Vec<SyncIssue> {
    compile(&snap(files)).expect_err("expected issues")
}

fn has_issue(issues: &[SyncIssue], file: &str, needle: &str) -> bool {
    issues
        .iter()
        .any(|i| i.file.as_deref() == Some(file) && i.reason.contains(needle))
}

fn read_dir(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            read_dir(root, &path, out);
        } else {
            let rel = path.strip_prefix(root).unwrap();
            let key: Vec<_> = rel.iter().map(|c| c.to_string_lossy()).collect();
            out.insert(key.join("/"), std::fs::read(&path).unwrap());
        }
    }
}

fn example_snapshot() -> Snapshot {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
    let mut files = BTreeMap::new();
    read_dir(&root, &root, &mut files);
    Snapshot {
        commit: "example".into(),
        files,
    }
}

fn host_json(extra: &str) -> String {
    format!(r#"{{"host":"web-01","groups":["production"]{extra}}}"#)
}

fn item<'a>(h: &'a CompiledHost, key: &str) -> &'a EffectiveItem {
    h.effective
        .items
        .iter()
        .find(|i| i.key == key && i.params.is_none())
        .unwrap()
}

#[test]
fn example_repo_compiles() {
    let c = compile(&example_snapshot()).unwrap_or_else(|i| panic!("{i:#?}"));
    assert_eq!(c.hosts.len(), 2);
    assert_eq!(c.documents.len(), 4);
}

#[test]
fn host_entry_beats_template() {
    let c = compile(&example_snapshot()).unwrap();
    let web = c.hosts.iter().find(|h| h.name == "web-01").unwrap();
    let cpu = item(web, "cpu.usage");
    assert_eq!((cpu.interval_secs, cpu.origin.as_str()), (10, "host"));
    let mem = item(web, "memory.used_percent");
    assert_eq!(
        (mem.interval_secs, mem.origin.as_str()),
        (30, "template:linux-base")
    );
}

const TPL_A: &str = r#"{"template":"a","items":[{"key":"cpu.usage","interval":"30s"}]}"#;
const TPL_B: &str = r#"{"template":"b","items":[{"key":"cpu.usage","interval":"60s"}]}"#;

#[test]
fn later_template_wins() {
    for (order, secs, origin) in [
        (r#","templates":["a","b"]"#, 60, "template:b"),
        (r#","templates":["b","a"]"#, 30, "template:a"),
    ] {
        let h = host_json(order);
        let c = compile(&snap(&[
            ("templates/a.json", TPL_A),
            ("templates/b.json", TPL_B),
            ("hosts/web-01.json", &h),
        ]))
        .unwrap();
        let cpu = item(&c.hosts[0], "cpu.usage");
        assert_eq!((cpu.interval_secs, cpu.origin.as_str()), (secs, origin));
    }
}

#[test]
fn specific_and_general_item_coexist() {
    let c = compile(&example_snapshot()).unwrap();
    let db = c.hosts.iter().find(|h| h.name == "db-01").unwrap();
    let disks: Vec<_> = db
        .effective
        .items
        .iter()
        .filter(|i| i.key == "disk.used_percent")
        .map(|i| {
            (
                i.params.as_ref().map(|p| p["mount"].clone()),
                i.interval_secs,
            )
        })
        .collect();
    assert_eq!(disks, [(None, 300), (Some("/".to_owned()), 60)]);
}

#[test]
fn trigger_override_by_name() {
    let tpl = r#"{"template":"a","items":[{"key":"disk.used_percent","interval":"5m"}],
        "triggers":[{"name":"Disk almost full","expression":"disk.used_percent > 90","severity":"high"}]}"#;
    let h = host_json(
        r#","templates":["a"],"triggers":[{"name":"Disk almost full","expression":"disk.used_percent > 95","severity":"critical"}]"#,
    );
    let c = compile(&snap(&[
        ("templates/a.json", tpl),
        ("hosts/web-01.json", &h),
    ]))
    .unwrap();
    let triggers = &c.hosts[0].effective.triggers;
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].expression, "disk.used_percent > 95");
    assert_eq!(triggers[0].severity, Severity::Critical);
    assert_eq!(triggers[0].origin, "host");
}

#[test]
fn syntax_error_has_position() {
    let issues = issues_of(&[("hosts/a.json", "{\n  \"host\": \"a\"\n  \"groups\": []\n}")]);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].file.as_deref(), Some("hosts/a.json"));
    assert_eq!(issues[0].line, Some(3));
    assert!(issues[0].column.is_some());
}

#[test]
fn unknown_item_key() {
    let h = host_json(r#","items":[{"key":"disk.usedpercent","interval":"1m"}]"#);
    let issues = issues_of(&[("hosts/a.json", &h)]);
    assert!(has_issue(
        &issues,
        "hosts/a.json",
        r#"unknown item key "disk.usedpercent""#
    ));
}

#[test]
fn wrong_param_for_item() {
    let h = host_json(
        r#","items":[{"key":"cpu.usage","params":{"mount":"/"},"interval":"1m"},
        {"key":"disk.used_percent","params":{"interface":"eth0"},"interval":"1m"}]"#,
    );
    let issues = issues_of(&[("hosts/a.json", &h)]);
    assert!(has_issue(&issues, "hosts/a.json", "takes no parameters"));
    assert!(has_issue(
        &issues,
        "hosts/a.json",
        r#"only accepts the parameter "mount""#
    ));
}

#[test]
fn item_param_value_too_long() {
    let long = "x".repeat(cerbrix_common::MAX_PARAM_VALUE_LEN + 1);
    let h = host_json(&format!(
        r#","items":[{{"key":"disk.used_percent","params":{{"mount":"{long}"}},"interval":"1m"}},
        {{"key":"disk.used_bytes","params":{{"mount":""}},"interval":"1m"}}]"#
    ));
    let issues = issues_of(&[("hosts/a.json", &h)]);
    assert!(has_issue(&issues, "hosts/a.json", "bytes long"));
    assert!(has_issue(&issues, "hosts/a.json", "empty value"));
}

#[test]
fn trigger_param_value_empty() {
    let tpl = r#"{"template":"a","triggers":[
        {"name":"t","expression":"disk.used_percent{mount=\"\"} > 90","severity":"high"}]}"#;
    let issues = issues_of(&[("templates/a.json", tpl)]);
    assert!(
        has_issue(&issues, "templates/a.json", "empty value"),
        "{issues:?}"
    );
}

#[test]
fn unknown_template() {
    let h = host_json(r#","templates":["nope"]"#);
    let issues = issues_of(&[("hosts/a.json", &h)]);
    assert!(has_issue(
        &issues,
        "hosts/a.json",
        r#"unknown template "nope""#
    ));
}

#[test]
fn template_listed_twice() {
    let h = host_json(r#","templates":["a","a"]"#);
    let issues = issues_of(&[("templates/a.json", TPL_A), ("hosts/a.json", &h)]);
    assert!(has_issue(&issues, "hosts/a.json", "listed more than once"));
}

#[test]
fn duplicate_host_name() {
    let h = host_json("");
    let issues = issues_of(&[("hosts/a.json", &h), ("hosts/x/b.json", &h)]);
    assert_eq!(issues.len(), 2);
    for i in &issues {
        assert!(i.reason.contains("hosts/a.json") && i.reason.contains("hosts/x/b.json"));
    }
}

#[test]
fn duplicate_template_name() {
    let issues = issues_of(&[("templates/a.json", TPL_A), ("templates/b.json", TPL_A)]);
    assert!(has_issue(
        &issues,
        "templates/a.json",
        "duplicate template name"
    ));
    assert!(has_issue(
        &issues,
        "templates/b.json",
        "duplicate template name"
    ));
}

#[test]
fn host_without_items_is_valid() {
    let c = compile(&snap(&[("hosts/a.json", &host_json(""))])).unwrap();
    assert!(c.hosts[0].effective.items.is_empty());
}

#[test]
fn trigger_on_item_the_host_lacks() {
    let tpl = r#"{"template":"a","triggers":[
        {"name":"Net down","expression":"net.rx_bytes_per_sec < 1","severity":"high"}]}"#;
    let h = host_json(r#","templates":["a"]"#);
    let issues = issues_of(&[("templates/a.json", tpl), ("hosts/web-01.json", &h)]);
    assert_eq!(issues.len(), 1);
    assert!(has_issue(&issues, "hosts/web-01.json", "Net down"));
}

#[test]
fn invalid_trigger_expression() {
    let h = host_json(
        r#","items":[{"key":"cpu.usage","interval":"1m"}],"triggers":[
        {"name":"t","expression":"cpu.usage >> 90","severity":"high"}]"#,
    );
    let issues = issues_of(&[("hosts/a.json", &h)]);
    assert!(
        has_issue(&issues, "hosts/a.json", "column 12"),
        "{issues:?}"
    );
}

#[test]
fn all_issues_are_collected() {
    let issues = issues_of(&[
        ("hosts/c.json", "{"),
        ("hosts/a.json", "[]"),
        ("templates/b.json", "nope"),
    ]);
    let files: Vec<_> = issues.iter().map(|i| i.file.clone().unwrap()).collect();
    assert_eq!(files, ["hosts/a.json", "hosts/c.json", "templates/b.json"]);

    let many: Vec<(String, &str)> = (0..60)
        .map(|i| (format!("hosts/h{i:02}.json"), "{"))
        .collect();
    let refs: Vec<(&str, &str)> = many.iter().map(|(p, c)| (p.as_str(), *c)).collect();
    assert_eq!(issues_of(&refs).len(), MAX_ISSUES);
}

#[test]
fn ignores_other_files() {
    let c = compile(&snap(&[
        ("README.md", "# hi"),
        ("hosts/notes.txt", "not json {"),
        ("scripts/x.json", "{"),
        ("hosts/.json", "{"),
        ("top.json", "{"),
    ]))
    .unwrap();
    assert!(c.hosts.is_empty() && c.documents.is_empty());
}

#[test]
fn empty_snapshot_is_valid() {
    let c = compile(&Snapshot::default()).unwrap();
    assert!(c.hosts.is_empty() && c.documents.is_empty());
}

#[test]
fn bom_and_invalid_utf8_rejected() {
    let mut s = Snapshot::default();
    s.files.insert(
        "hosts/a.json".into(),
        b"\xEF\xBB\xBF{\"host\":\"a\"}".to_vec(),
    );
    s.files
        .insert("hosts/b.json".into(), b"{\"host\":\"\xFF\"}".to_vec());
    let issues = compile(&s).unwrap_err();
    assert!(has_issue(&issues, "hosts/a.json", "byte order mark"));
    assert!(has_issue(&issues, "hosts/b.json", "not valid UTF-8"));
}

#[test]
fn invalid_names_and_notifications() {
    let h = r#"{"host":"bad host","groups":["Bad"]}"#;
    let t = r#"{"template":"Bad"}"#;
    let n = r#"{"name":"n","email":["a@b","@b","a@","a@@b","ab"],"min_severity":"info"}"#;
    let e = r#"{"name":"e","email":[],"min_severity":"info"}"#;
    let issues = issues_of(&[
        ("hosts/a.json", h),
        ("templates/t.json", t),
        ("notifications/n.json", n),
        ("notifications/e.json", e),
    ]);
    assert!(has_issue(&issues, "hosts/a.json", "invalid host name"));
    assert!(has_issue(&issues, "hosts/a.json", "group name"));
    assert!(has_issue(&issues, "templates/t.json", "template name"));
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.file.as_deref() == Some("notifications/n.json"))
            .count(),
        4
    );
    assert!(has_issue(&issues, "notifications/e.json", "empty"));
}

#[test]
fn never_panics_on_overflowing_placement() {
    let d = r#"{"name":"d","panels":[{"type":"problems","x":250,"y":250,"w":250,"h":250}]}"#;
    let issues = issues_of(&[("dashboards/d.json", d)]);
    assert!(has_issue(&issues, "dashboards/d.json", "x + w is 500"));
}

// Boards

fn dashboard(panels: &str) -> String {
    format!(r#"{{"name":"d","hosts":{{"groups":["production"]}},"panels":[{panels}]}}"#)
}

fn with_example(extra: &[(&str, &str)]) -> Vec<SyncIssue> {
    let mut s = example_snapshot();
    for (p, c) in extra {
        s.files.insert((*p).to_owned(), c.as_bytes().to_vec());
    }
    compile(&s).expect_err("expected issues")
}

#[test]
fn overlapping_panels() {
    let d = dashboard(
        r#"{"type":"problems","x":0,"y":0,"w":6,"h":3},{"type":"host-states","x":5,"y":2,"w":4,"h":2}"#,
    );
    let issues = issues_of(&[("dashboards/d.json", &d)]);
    assert!(has_issue(
        &issues,
        "dashboards/d.json",
        "panels 1 and 2 overlap"
    ));
}

#[test]
fn touching_panels_do_not_overlap() {
    let d = dashboard(
        r#"{"type":"problems","x":0,"y":0,"w":6,"h":6},{"type":"host-states","x":6,"y":0,"w":6,"h":6}"#,
    );
    assert!(compile(&snap(&[("dashboards/d.json", &d)])).is_ok());
}

#[test]
fn panel_outside_grid() {
    let d = dashboard(
        r#"{"type":"problems","x":10,"y":0,"w":3,"h":1},{"type":"host-states","x":0,"y":5,"w":1,"h":2}"#,
    );
    let issues = issues_of(&[("dashboards/d.json", &d)]);
    assert!(has_issue(&issues, "dashboards/d.json", "x + w is 13"));
    assert!(has_issue(&issues, "dashboards/d.json", "y + h is 7"));
}

#[test]
fn zero_sized_panel() {
    let d = dashboard(r#"{"type":"problems","x":0,"y":0,"w":0,"h":0}"#);
    let issues = issues_of(&[("dashboards/d.json", &d)]);
    assert!(has_issue(&issues, "dashboards/d.json", "at least 1"));
}

#[test]
fn graph_host_not_selected() {
    let d = r#"{"name":"d","hosts":{"groups":["web"]},"panels":[
        {"type":"value","host":"db-01","item":"cpu.usage","x":0,"y":0,"w":2,"h":2}]}"#;
    let issues = with_example(&[("dashboards/d.json", d)]);
    assert!(has_issue(&issues, "dashboards/d.json", "not selected"));
}

#[test]
fn graph_item_not_on_host() {
    let d = dashboard(
        r#"{"type":"graph","host":"db-01","item":"net.rx_bytes_per_sec","x":0,"y":0,"w":2,"h":2},
        {"type":"value","host":"ghost","item":"cpu.usage","x":4,"y":0,"w":2,"h":2}"#,
    );
    let issues = with_example(&[("dashboards/d.json", &d)]);
    assert!(has_issue(
        &issues,
        "dashboards/d.json",
        "does not collect item"
    ));
    assert!(has_issue(
        &issues,
        "dashboards/d.json",
        r#"host "ghost" does not exist"#
    ));
}

#[test]
fn duplicate_dashboard_stem_in_subdirectories() {
    let d = |n: &str| format!(r#"{{"name":"{n}"}}"#);
    let issues = issues_of(&[
        ("dashboards/a/x.json", &d("one")),
        ("dashboards/b/x.json", &d("two")),
    ]);
    assert_eq!(issues.len(), 2);
    assert!(has_issue(
        &issues,
        "dashboards/a/x.json",
        "dashboards/b/x.json"
    ));
}

#[test]
fn dashboard_id_and_name_rules() {
    let issues = issues_of(&[
        ("dashboards/Bad Id.json", r#"{"name":"same"}"#),
        ("dashboards/ok.json", r#"{"name":"same"}"#),
    ]);
    assert!(has_issue(&issues, "dashboards/Bad Id.json", "dashboard id"));
    assert!(has_issue(
        &issues,
        "dashboards/ok.json",
        "duplicate dashboard name"
    ));
}

fn display(show: &str, rest: &str) -> String {
    format!(r#"{{"name":"x","show":{show}{rest}}}"#)
}

#[test]
fn display_unknown_dashboard() {
    let d = display(r#"{"dashboard":"nope"}"#, "");
    let issues = with_example(&[("displays/x.json", &d)]);
    assert!(has_issue(
        &issues,
        "displays/x.json",
        r#"dashboard "nope" does not exist"#
    ));
}

#[test]
fn display_show_outside_allowed_list() {
    let d = display(
        r#"{"dashboard":"production"}"#,
        r#","dashboards":["other","gone"]"#,
    );
    let issues = with_example(&[
        ("displays/x.json", &d),
        ("dashboards/other.json", r#"{"name":"Other"}"#),
    ]);
    assert!(has_issue(
        &issues,
        "displays/x.json",
        "not in the \"dashboards\" list"
    ));
    assert!(has_issue(&issues, "displays/x.json", r#"lists "gone""#));
}

#[test]
fn display_empty_rotation() {
    let d = display(r#"{"rotate":[],"every":"30s"}"#, "");
    let issues = issues_of(&[("displays/x.json", &d)]);
    assert!(has_issue(&issues, "displays/x.json", "\"rotate\" is empty"));
}

#[test]
fn display_rotation_too_fast() {
    let d = display(r#"{"rotate":["production"],"every":"1s"}"#, "");
    let issues = with_example(&[("displays/x.json", &d)]);
    assert!(has_issue(&issues, "displays/x.json", "between 5s and 1h"));
    let d = display(r#"{"rotate":["production"],"every":"2h"}"#, "");
    let issues = with_example(&[("displays/x.json", &d)]);
    assert!(has_issue(&issues, "displays/x.json", "between 5s and 1h"));
}

// Bounded work on hostile input

#[test]
fn too_many_panels_is_one_issue() {
    let panel = r#"{"type":"problems","x":0,"y":0,"w":1,"h":1}"#;
    let panels = vec![panel; 10_000].join(",");
    let d = format!(r#"{{"name":"d","panels":[{panels}]}}"#);
    let started = std::time::Instant::now();
    let issues = issues_of(&[("dashboards/d.json", &d)]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(
        has_issue(&issues, "dashboards/d.json", "10000 panels")
            && has_issue(&issues, "dashboards/d.json", "at most 72"),
        "{issues:?}"
    );
}

#[test]
fn seventy_two_panels_fit() {
    let mut panels = Vec::new();
    for y in 0..6 {
        for x in 0..12 {
            panels.push(format!(
                r#"{{"type":"problems","x":{x},"y":{y},"w":1,"h":1}}"#
            ));
        }
    }
    let d = format!(r#"{{"name":"d","panels":[{}]}}"#, panels.join(","));
    assert!(compile(&snap(&[("dashboards/d.json", &d)])).is_ok());
}

#[test]
fn many_duplicate_names_stay_bounded() {
    let files: Vec<(String, String)> = (0..2_000)
        .map(|i| (format!("hosts/h{i:04}.json"), host_json("")))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let issues = issues_of(&refs);
    assert!(issues.len() <= MAX_ISSUES, "{}", issues.len());
    for issue in &issues {
        assert!(issue.reason.len() < 1_000, "{}", issue.reason);
        assert!(issue.reason.contains("and 1995 more"), "{}", issue.reason);
    }
}

#[test]
fn too_many_templates_is_one_issue() {
    let list = vec![r#""a""#; 100_000].join(",");
    let h = host_json(&format!(r#","templates":[{list}]"#));
    let started = std::time::Instant::now();
    let issues = issues_of(&[("templates/a.json", TPL_A), ("hosts/a.json", &h)]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(
        has_issue(&issues, "hosts/a.json", "at most 32 templates"),
        "{issues:?}"
    );
}

#[test]
fn thirty_two_templates_are_allowed() {
    let mut files = Vec::new();
    for i in 0..32 {
        files.push((
            format!("templates/t{i}.json"),
            format!(r#"{{"template":"t{i}"}}"#),
        ));
    }
    let names: Vec<String> = (0..32).map(|i| format!(r#""t{i}""#)).collect();
    files.push((
        "hosts/a.json".to_owned(),
        host_json(&format!(r#","templates":[{}]"#, names.join(","))),
    ));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let c = compile(&snap(&refs)).unwrap_or_else(|i| panic!("{i:#?}"));
    assert_eq!(c.hosts[0].effective.templates.len(), 32);
}

#[test]
fn overlap_reports_first_pair_only() {
    let d = dashboard(
        r#"{"type":"problems","x":0,"y":0,"w":2,"h":2},
        {"type":"host-states","x":0,"y":0,"w":2,"h":2},
        {"type":"problems","x":1,"y":1,"w":2,"h":2}"#,
    );
    let issues = issues_of(&[("dashboards/d.json", &d)]);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].reason, "panels 1 and 2 overlap");
}

// Control characters

#[test]
fn control_character_in_any_string_is_an_issue() {
    let cases = [
        (
            "hosts/a.json",
            r#"{"host":"a","address":"10.0.0.1\u0000"}"#.to_owned(),
            "\"address\"",
        ),
        (
            "hosts/b.json",
            r#"{"host":"b","items":[{"key":"cpu.usage","interval":"1m"}],
                "triggers":[{"name":"CPU\nhigh","expression":"cpu.usage > 90","severity":"high"}]}"#
                .to_owned(),
            "\"triggers[0].name\"",
        ),
        (
            "hosts/c.json",
            // A raw tab is already a JSON syntax error; the escaped one is valid JSON.
            r#"{"host":"c","groups":["prod\tuction"]}"#.to_owned(),
            "\"groups[0]\"",
        ),
        (
            "templates/t.json",
            r#"{"template":"t","items":[{"key":"disk.used_percent","params":{"mo\u001bunt":"/"},"interval":"1m"}]}"#
                .to_owned(),
            "the key at \"items[0].params.mo?unt\"",
        ),
    ];
    for (file, content, needle) in &cases {
        let issues = issues_of(&[(file, content)]);
        assert_eq!(issues.len(), 1, "{file}: {issues:?}");
        assert!(has_issue(&issues, file, "control character"), "{issues:?}");
        assert!(has_issue(&issues, file, needle), "{issues:?}");
        assert!(
            !issues[0].reason.chars().any(char::is_control),
            "{issues:?}"
        );
    }
}

#[test]
fn printable_unicode_is_accepted() {
    let h = r#"{"host":"a","address":"Bürogebäude 3 – Raum 7"}"#;
    assert!(compile(&snap(&[("hosts/a.json", h)])).is_ok());
}

#[test]
fn agent_config_version_changes_with_items() {
    let make = |interval: &str| {
        let h = host_json(&format!(
            r#","items":[{{"key":"cpu.usage","interval":"{interval}"}}]"#
        ));
        let c = compile(&snap(&[("hosts/a.json", &h)])).unwrap();
        c.hosts[0].effective.agent_config()
    };
    let (a, b, c) = (make("30s"), make("30s"), make("60s"));
    assert_eq!(a, b);
    assert_ne!(a.version, c.version);
    assert_eq!(a.version.len(), 64);
    assert!(
        a.version
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(a.items.len(), 1);
}
