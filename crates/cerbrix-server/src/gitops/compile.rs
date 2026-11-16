//! Parsing a snapshot, merging templates into hosts and cross-checking.

use std::collections::{BTreeMap, BTreeSet};

use cerbrix_common::{
    ConfiguredItem, HostFile, ItemConfig, ItemEntry, MAX_PARAM_VALUE_LEN, NotificationFile,
    Selector, Severity, TemplateFile, TriggerEntry, TriggerExpr, builtin_item, parse_trigger,
    validate_host_name, validate_name,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Issues, MAX_ISSUES, Snapshot, SyncIssue, boards, replace_control};

type Params = BTreeMap<String, String>;

/// An item a host collects after merging, with where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveItem {
    pub key: String,
    pub params: Option<Params>,
    pub interval_secs: u64,
    /// `host` or `template:<name>`.
    pub origin: String,
}

/// A trigger a host evaluates after merging, with where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveTrigger {
    pub name: String,
    pub expression: String,
    pub severity: Severity,
    /// `host` or `template:<name>`.
    pub origin: String,
}

/// A host's configuration after applying its templates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveHost {
    pub templates: Vec<String>,
    pub items: Vec<EffectiveItem>,
    pub triggers: Vec<EffectiveTrigger>,
}

impl EffectiveHost {
    /// What the agent fetches: the items, versioned by the SHA-256 of their JSON.
    pub fn agent_config(&self) -> ItemConfig {
        let items: Vec<ConfiguredItem> = self
            .items
            .iter()
            .map(|i| ConfiguredItem {
                key: i.key.clone(),
                params: i.params.clone(),
                interval_secs: i.interval_secs,
            })
            .collect();
        item_config(items)
    }
}

/// Wraps items into what an agent fetches, versioned by the lower-case hex SHA-256 of their
/// JSON. Stored and default configurations share this, so equal items have equal versions.
pub fn item_config(items: Vec<ConfiguredItem>) -> ItemConfig {
    let json = serde_json::to_vec(&items).unwrap_or_default();
    let version = Sha256::digest(&json)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ItemConfig { version, items }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledHost {
    pub name: String,
    pub address: Option<String>,
    pub groups: Vec<String>,
    pub source_file: String,
    pub effective: EffectiveHost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Template,
    Notification,
    Dashboard,
    Display,
}

impl DocumentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DocumentKind::Template => "template",
            DocumentKind::Notification => "notification",
            DocumentKind::Dashboard => "dashboard",
            DocumentKind::Display => "display",
        }
    }
}

/// A template, notification, dashboard or display as stored by the server.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub kind: DocumentKind,
    pub id: String,
    pub source_file: String,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Compiled {
    pub hosts: Vec<CompiledHost>,
    pub documents: Vec<Document>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Hosts,
    Templates,
    Notifications,
    Dashboards,
    Displays,
}

/// The directory a path belongs to, if it is a configuration file.
fn classify(path: &str) -> Option<Dir> {
    let (dir, rest) = path.split_once('/')?;
    if rest.len() <= ".json".len() || !rest.ends_with(".json") {
        return None;
    }
    match dir {
        "hosts" => Some(Dir::Hosts),
        "templates" => Some(Dir::Templates),
        "notifications" => Some(Dir::Notifications),
        "dashboards" => Some(Dir::Dashboards),
        "displays" => Some(Dir::Displays),
        _ => None,
    }
}

/// A host may list at most this many templates.
pub const MAX_TEMPLATES_PER_HOST: usize = 32;
/// A duplicate-name issue lists at most this many of the files involved.
const MAX_LISTED_FILES: usize = 5;
/// Names and paths quoted in a reason are cut to this many characters.
const MAX_QUOTED_CHARS: usize = 100;

/// `text` cut to [`MAX_QUOTED_CHARS`] characters, with control characters replaced.
pub(super) fn short(text: &str) -> String {
    let mut out: String = replace_control(text)
        .chars()
        .take(MAX_QUOTED_CHARS)
        .collect();
    if text.chars().nth(MAX_QUOTED_CHARS).is_some() {
        out.push_str("...");
    }
    out
}

fn json_issue(path: &str, e: &serde_json::Error) -> SyncIssue {
    let full = e.to_string();
    let reason = full
        .rsplit_once(" at line ")
        .map_or(full.as_str(), |(head, _)| head);
    let mut issue = SyncIssue::new(Some(path), format!("invalid JSON: {reason}"));
    if e.line() > 0 {
        issue.line = u32::try_from(e.line()).ok();
        issue.column = u32::try_from(e.column()).ok();
    }
    issue
}

/// Where the first key or string value containing a control character is, if any.
/// Checked once on the untyped value, so no field of any file can be forgotten.
fn control_character_at(value: &serde_json::Value) -> Option<String> {
    fn shown(path: &str) -> String {
        if path.is_empty() {
            "the top level".to_owned()
        } else {
            format!("{:?}", short(path))
        }
    }
    fn walk(value: &serde_json::Value, path: &mut String) -> Option<String> {
        use serde_json::Value;
        match value {
            Value::String(s) if s.chars().any(char::is_control) => {
                Some(format!("the value at {}", shown(path)))
            }
            Value::Array(values) => {
                for (i, v) in values.iter().enumerate() {
                    let len = path.len();
                    path.push_str(&format!("[{i}]"));
                    if let Some(found) = walk(v, path) {
                        return Some(found);
                    }
                    path.truncate(len);
                }
                None
            }
            Value::Object(map) => {
                for (key, v) in map {
                    let len = path.len();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(&replace_control(key));
                    if key.chars().any(char::is_control) {
                        return Some(format!("the key at {}", shown(path)));
                    }
                    if let Some(found) = walk(v, path) {
                        return Some(found);
                    }
                    path.truncate(len);
                }
                None
            }
            _ => None,
        }
    }
    walk(value, &mut String::new())
}

/// Parses one file, reporting a BOM, bad UTF-8, a control character in a string
/// or a JSON/type error with its position.
fn load<T: DeserializeOwned>(path: &str, bytes: &[u8], issues: &mut Issues) -> Option<T> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        issues.add(Some(path), || {
            "file starts with a byte order mark (BOM): save it as UTF-8 without BOM".to_owned()
        });
        return None;
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => {
            issues.add(Some(path), || {
                format!(
                    "file is not valid UTF-8 (invalid byte at offset {})",
                    e.valid_up_to()
                )
            });
            return None;
        }
    };
    // Untyped first: syntax errors keep their position, and every string is checked.
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            issues.add_issue(|| json_issue(path, &e));
            return None;
        }
    };
    if let Some(at) = control_character_at(&value) {
        issues.add(Some(path), || {
            format!(
                "{at} contains a control character (for example NUL, a tab or a line break): remove it"
            )
        });
        return None;
    }
    drop(value);
    // Parsed again from the text so that type errors keep their position too.
    match serde_json::from_str(text) {
        Ok(v) => Some(v),
        Err(e) => {
            issues.add_issue(|| json_issue(path, &e));
            None
        }
    }
}

/// Reports every file of a group of entries that share one name.
pub(super) fn duplicates<'a>(
    what: &str,
    entries: impl IntoIterator<Item = (&'a str, &'a str)>,
    issues: &mut Issues,
) {
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, file) in entries {
        groups.entry(name).or_default().push(file);
    }
    for (name, files) in groups.into_iter().filter(|(_, f)| f.len() > 1) {
        if issues.is_full() {
            return;
        }
        let mut list = files
            .iter()
            .take(MAX_LISTED_FILES)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        if files.len() > MAX_LISTED_FILES {
            list.push_str(&format!(" and {} more", files.len() - MAX_LISTED_FILES));
        }
        let name = short(name);
        for file in &files {
            if issues.is_full() {
                return;
            }
            issues.add(Some(file), || {
                format!("duplicate {what} {name:?}: used by {list}")
            });
        }
    }
}

/// Problems with the `params` of an item `key` (unknown keys are reported elsewhere).
pub(super) fn param_problems(key: &str, params: &Params) -> Vec<String> {
    let mut out = Vec::new();
    let Some(spec) = builtin_item(key) else {
        return out;
    };
    for (name, value) in params {
        match spec.param {
            None => out.push(format!(
                "item {key:?} takes no parameters, but params has {name:?}"
            )),
            Some(allowed) if allowed != name => out.push(format!(
                "item {key:?} only accepts the parameter {allowed:?}, not {name:?}"
            )),
            Some(_) => {}
        }
        if value.is_empty() {
            out.push(format!(
                "parameter {name:?} of item {key:?} has an empty value"
            ));
        } else if value.len() > MAX_PARAM_VALUE_LEN {
            out.push(format!(
                "parameter {name:?} of item {key:?} is {} bytes long: the limit is {MAX_PARAM_VALUE_LEN}",
                value.len()
            ));
        }
    }
    out
}

fn check_items(file: &str, items: &[ItemEntry], issues: &mut Issues) {
    for item in items {
        if builtin_item(&item.key).is_none() {
            issues.add(Some(file), || {
                format!(
                    "unknown item key {:?}: the built-in items are listed in docs/items.md",
                    item.key
                )
            });
        } else if let Some(params) = &item.params
            && !issues.is_full()
        {
            for problem in param_problems(&item.key, params) {
                issues.add(Some(file), || problem);
            }
        }
    }
}

/// The selector of a parsed trigger expression, if it has one.
fn selector_of(expr: &TriggerExpr) -> Option<&Selector> {
    match expr {
        TriggerExpr::Compare { selector, .. } => Some(selector),
        TriggerExpr::NoData { selector, .. } => selector.as_ref(),
    }
}

fn check_triggers(file: &str, triggers: &[TriggerEntry], issues: &mut Issues) {
    for trigger in triggers {
        if issues.is_full() {
            return;
        }
        let name = &trigger.name;
        match parse_trigger(&trigger.expression) {
            Err(e) => issues.add(Some(file), || {
                format!("trigger {name:?}: invalid expression, {e}")
            }),
            Ok(expr) => {
                let Some(selector) = selector_of(&expr) else {
                    continue;
                };
                if builtin_item(&selector.key).is_none() {
                    issues.add(Some(file), || {
                        format!(
                            "trigger {name:?}: unknown item key {:?}: the built-in items are listed in docs/items.md",
                            selector.key
                        )
                    });
                } else {
                    for problem in param_problems(&selector.key, &selector.params) {
                        issues.add(Some(file), || format!("trigger {name:?}: {problem}"));
                    }
                }
            }
        }
    }
}

fn check_group_names(file: &str, groups: &[String], issues: &mut Issues) {
    for group in groups {
        if let Err(e) = validate_name(group) {
            issues.add(Some(file), || format!("group name: {e}"));
        }
    }
}

type Identity = (String, Option<Params>);

fn identity(entry: &ItemEntry) -> Identity {
    // An empty `params` object means the same as none.
    let params = entry.params.clone().filter(|p| !p.is_empty());
    (entry.key.clone(), params)
}

/// Applies the templates in the listed order, then the host's own entries.
fn merge(host: &HostFile, templates: &BTreeMap<&str, &TemplateFile>) -> EffectiveHost {
    let mut items: BTreeMap<Identity, EffectiveItem> = BTreeMap::new();
    let mut triggers: BTreeMap<String, EffectiveTrigger> = BTreeMap::new();
    let mut apply = |entries: &[ItemEntry], trigger_entries: &[TriggerEntry], origin: String| {
        for entry in entries {
            let id = identity(entry);
            items.insert(
                id.clone(),
                EffectiveItem {
                    key: id.0,
                    params: id.1,
                    interval_secs: entry.interval.as_secs(),
                    origin: origin.clone(),
                },
            );
        }
        for t in trigger_entries {
            triggers.insert(
                t.name.clone(),
                EffectiveTrigger {
                    name: t.name.clone(),
                    expression: t.expression.clone(),
                    severity: t.severity,
                    origin: origin.clone(),
                },
            );
        }
    };
    for name in &host.templates {
        if let Some(t) = templates.get(name.as_str()) {
            apply(&t.items, &t.triggers, format!("template:{name}"));
        }
    }
    apply(&host.items, &host.triggers, "host".to_owned());
    EffectiveHost {
        templates: host.templates.clone(),
        items: items.into_values().collect(),
        triggers: triggers.into_values().collect(),
    }
}

/// Checks a host's `templates` list before anything is merged. Returns whether the
/// list is short enough to merge at all.
fn check_template_list(
    file: &str,
    host: &HostFile,
    templates: &BTreeMap<&str, &TemplateFile>,
    issues: &mut Issues,
) -> bool {
    if host.templates.len() > MAX_TEMPLATES_PER_HOST {
        issues.add(Some(file), || {
            format!(
                "\"templates\" lists {} entries: a host may list at most {MAX_TEMPLATES_PER_HOST} templates",
                host.templates.len()
            )
        });
        return false;
    }
    let mut seen = BTreeSet::new();
    for name in &host.templates {
        if !templates.contains_key(name.as_str()) {
            issues.add(Some(file), || {
                format!("unknown template {name:?}: there is no file in templates/ that defines it")
            });
        } else if !seen.insert(name) {
            issues.add(Some(file), || {
                format!("template {name:?} is listed more than once")
            });
        }
    }
    true
}

/// Checks a host file. `effective` is `None` when its templates were not merged.
fn check_host(file: &str, host: &HostFile, effective: Option<&EffectiveHost>, issues: &mut Issues) {
    if let Err(e) = validate_host_name(&host.host) {
        issues.add(Some(file), || e.to_string());
    }
    check_group_names(file, &host.groups, issues);
    check_items(file, &host.items, issues);
    check_triggers(file, &host.triggers, issues);

    let Some(effective) = effective else {
        return;
    };
    // A trigger can only watch an item the host actually collects. This depends
    // on the host, so it is reported at the host file even for template triggers.
    let keys: BTreeSet<&str> = effective.items.iter().map(|i| i.key.as_str()).collect();
    for trigger in &effective.triggers {
        if issues.is_full() {
            return;
        }
        let Ok(expr) = parse_trigger(&trigger.expression) else {
            continue; // reported at the file that defines the trigger
        };
        if let Some(selector) = selector_of(&expr)
            && !keys.contains(selector.key.as_str())
            && builtin_item(&selector.key).is_some()
        {
            issues.add(Some(file), || {
                format!(
                    "trigger {:?} (from {}) uses item {:?}, which this host does not collect: add the item to the host or one of its templates",
                    trigger.name, trigger.origin, selector.key
                )
            });
        }
    }

    // A5: while a host is silent as a whole only a host-level no-data trigger can fire, so
    // an item-level one without it would leave an agent outage without any problem.
    let nodata: Vec<(&EffectiveTrigger, bool)> = effective
        .triggers
        .iter()
        .filter_map(|t| match parse_trigger(&t.expression) {
            Ok(TriggerExpr::NoData { selector, .. }) => Some((t, selector.is_some())),
            _ => None,
        })
        .collect();
    if !nodata.iter().any(|(_, item_level)| !item_level) {
        for (trigger, _) in nodata.iter().filter(|(_, item_level)| *item_level) {
            issues.add(Some(file), || {
                format!(
                    "trigger {:?} watches one item for missing data; add a host-level trigger such as \"no data for 5m\" so that an agent outage is one problem",
                    trigger.name
                )
            });
        }
    }
}

fn check_notification(file: &str, n: &NotificationFile, issues: &mut Issues) {
    if n.email.is_empty() {
        issues.add(Some(file), || {
            "\"email\" is empty: list at least one address".to_owned()
        });
    }
    for address in &n.email {
        let ok = address
            .split_once('@')
            .is_some_and(|(l, r)| !l.is_empty() && !r.is_empty() && !r.contains('@'));
        if !ok {
            issues.add(Some(file), || {
                format!(
                    "invalid email address {address:?}: it needs exactly one '@' with text on both sides"
                )
            });
        }
    }
    check_group_names(file, &n.groups, issues);
}

fn body<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// Turns a snapshot into the effective configuration, or into every issue found.
///
/// Only when this returns `Ok` may the server apply the revision.
pub fn compile(snapshot: &Snapshot) -> Result<Compiled, Vec<SyncIssue>> {
    let mut issues = Issues::new();
    let mut hosts: Vec<(String, HostFile)> = Vec::new();
    let mut templates: Vec<(String, TemplateFile)> = Vec::new();
    let mut notifications: Vec<(String, NotificationFile)> = Vec::new();
    let mut dashboards = Vec::new();
    let mut displays = Vec::new();

    for (path, bytes) in &snapshot.files {
        let Some(dir) = classify(path) else { continue };
        let file = path.clone();
        match dir {
            Dir::Hosts => hosts.extend(load(path, bytes, &mut issues).map(|h| (file, h))),
            Dir::Templates => templates.extend(load(path, bytes, &mut issues).map(|t| (file, t))),
            Dir::Notifications => {
                notifications.extend(load(path, bytes, &mut issues).map(|n| (file, n)));
            }
            Dir::Dashboards => dashboards.extend(load(path, bytes, &mut issues).map(|d| (file, d))),
            Dir::Displays => displays.extend(load(path, bytes, &mut issues).map(|d| (file, d))),
        }
    }

    duplicates(
        "host name",
        hosts.iter().map(|(f, h)| (h.host.as_str(), f.as_str())),
        &mut issues,
    );
    duplicates(
        "template name",
        templates
            .iter()
            .map(|(f, t)| (t.template.as_str(), f.as_str())),
        &mut issues,
    );
    duplicates(
        "notification name",
        notifications
            .iter()
            .map(|(f, n)| (n.name.as_str(), f.as_str())),
        &mut issues,
    );

    let mut documents = Vec::new();
    for (file, t) in &templates {
        if let Err(e) = validate_name(&t.template) {
            issues.add(Some(file), || format!("template name: {e}"));
        }
        check_items(file, &t.items, &mut issues);
        check_triggers(file, &t.triggers, &mut issues);
        documents.push(Document {
            kind: DocumentKind::Template,
            id: t.template.clone(),
            source_file: file.clone(),
            body: body(t),
        });
    }
    for (file, n) in &notifications {
        check_notification(file, n, &mut issues);
        documents.push(Document {
            kind: DocumentKind::Notification,
            id: n.name.clone(),
            source_file: file.clone(),
            body: body(n),
        });
    }

    let by_name: BTreeMap<&str, &TemplateFile> = templates
        .iter()
        .map(|(_, t)| (t.template.as_str(), t))
        .collect();
    let mut compiled_hosts = Vec::new();
    for (file, h) in &hosts {
        // The list is checked first: a host that lists too many templates is not merged.
        let effective = if check_template_list(file, h, &by_name, &mut issues) {
            let effective = merge(h, &by_name);
            check_host(file, h, Some(&effective), &mut issues);
            effective
        } else {
            check_host(file, h, None, &mut issues);
            EffectiveHost {
                templates: Vec::new(),
                items: Vec::new(),
                triggers: Vec::new(),
            }
        };
        compiled_hosts.push(CompiledHost {
            name: h.host.clone(),
            address: h.address.clone(),
            groups: h.groups.clone(),
            source_file: file.clone(),
            effective,
        });
    }

    documents.extend(boards::check(
        &dashboards,
        &displays,
        &compiled_hosts,
        &mut issues,
    ));

    if issues.is_empty() {
        documents.sort_by(|a, b| (a.kind.as_str(), &a.id).cmp(&(b.kind.as_str(), &b.id)));
        compiled_hosts.sort_by(|a, b| a.name.cmp(&b.name));
        return Ok(Compiled {
            hosts: compiled_hosts,
            documents,
        });
    }
    let mut issues = issues.into_vec();
    issues.sort_by(|a, b| {
        (&a.file, &a.reason, a.line, a.column).cmp(&(&b.file, &b.reason, b.line, b.column))
    });
    issues.dedup();
    issues.truncate(MAX_ISSUES);
    Err(issues)
}
