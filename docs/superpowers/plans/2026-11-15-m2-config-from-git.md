# M2 – Configuration from Git Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Hosts, templates, notifications, dashboards and displays are JSON files in a Git repository; the server fetches it on a cycle, applies a revision only if all of it is valid, reports Out Of Sync with file and reason otherwise, and agents measure what the repository says.

**Architecture:** The file formats live in `cerbrix-common` as serde types; the JSON Schemas are generated from those types and committed, so server and editors validate against the same source. The server gets a `gitops` module with four separable steps: fetch a snapshot (the `git` program behind a trait), compile it (pure function: parse, merge templates, cross-check), apply it (one PostgreSQL transaction), and a loop that ties them together under an advisory lock. The agent polls a new config endpoint, caches the answer on disk and rebuilds its scheduler when it changes.

**Tech Stack:** Rust 2024, serde, schemars (schema generation), jsonschema (tests only), the `git` and `ssh` programs, sqlx/PostgreSQL, axum, tokio.

**Spec:** `docs/superpowers/specs/2026-11-14-cerbrix-design.md` (sections 5.2, 5.5–5.7) and `docs/superpowers/specs/2026-11-15-ui-design-concept.md` (section 5, for dashboard and display files). Issues #16–#23 and #62.

## Global Constraints

- License Apache-2.0; English for code, comments, documentation and commit messages.
- `unsafe_code = "forbid"`. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace` and `cargo test --workspace` pass after every task.
- No server state only in process memory: sync state, active configuration and history live in PostgreSQL. A second server instance must see the same state.
- The Git sync runs on at most one instance at a time.
- All or nothing: an invalid revision changes nothing; the last valid configuration stays active.
- GitOps is one switch for the whole installation. No mixed mode.
- The server never connects to a host; the agent fetches its configuration.
- Secrets (Git token, SSH key path contents, enrolment keys, credentials) never appear in logs, error messages, `Debug` output or the database in plain text.
- Severities are exactly `info`, `warning`, `high`, `critical`.
- Dependencies are added with `cargo add` at the latest stable version; versions shared by several crates go in `[workspace.dependencies]`.
- Database tests need `DATABASE_URL=postgres://cerbrix:cerbrix@localhost:5432/cerbrix` (`docker compose -f docker-compose.dev.yml up -d`). Tests that need the `git` program may assume it is on `PATH`.

## Decisions this plan makes

The specs leave these open. Each is the author's call and is listed so the project owner can overrule it.

| # | Decision | Why |
|---|---|---|
| P1 | The trigger expression parser (issue #24, planned for M3) is built in M2 | Issue #19 requires valid trigger expressions for a revision to apply |
| P2 | Git access shells out to the `git` program instead of linking a Git library | Token and SSH authentication behave exactly as operators know them; the server image gains `git` and `openssh-client` |
| P3 | Files may sit in subdirectories: `hosts/**/*.json` and likewise for the other four roots | The UI concept shows `hosts/kunde-a/db-02.json`; flat directories do not scale to many customers |
| P4 | Item keys are checked against the built-in catalogue; an unknown key makes the revision invalid | A typo otherwise yields a host that silently measures nothing |
| P5 | A revision that would remove more than 50 % of the active hosts (when at least 10 are active) is not applied and reports Out Of Sync; `gitops.max_host_removal_percent = 100` turns the guard off | Removing a host revokes its agent credential. A wrong branch or an emptied directory would otherwise force a re-enrolment of every host |
| P6 | With GitOps off, hosts added by `admin host add` get every built-in item at the M1 default intervals | Managing items in the UI is v0.2; without this such a host would measure nothing |
| P7 | The agent's `[intervals]` setting is removed; exclusion patterns for disks and interfaces stay local | Issue #21: items and intervals come from the server |
| P8 | SSH host keys: `gitops.known_hosts_file` if set, otherwise trust on first use, remembered in the Git cache directory | Works out of the box; operators who need pinning can supply the file |
| P9 | Sync state and history are readable through the store and `cerbrix-server admin sync …`; the HTTP endpoint for the dashboard comes with the UI API in M4 (#31) | There is no user login before M4, so an HTTP endpoint would be unauthenticated |
| P10 | A notification file is `{ "name", "email": [..], "min_severity", "groups": [..] }` | The spec only says "who gets an e-mail for which severity and group"; M3 consumes it |
| P11 | "Open problems are closed" for a removed host (#22) is left to M3 | Problems do not exist before M3; this plan deactivates the host and revokes its credential |

## Review Focus

1. A symlink, a file over 1 MiB, invalid UTF-8 or a UTF-8 BOM in the repository: the revision is rejected with that file and a clear reason, and nothing outside the checkout is ever read. → Task 6.
2. `git` prints a URL or a credential on failure: neither the stored reason nor the log contains the token. → Task 8.
3. A host is removed and later added again: it is active again, the old credential stays revoked, the agent must enrol anew. → Task 7.
4. The same invalid commit is fetched every 60 seconds: one Out Of Sync state, no growing history; and a fetch failure followed by an unchanged valid commit clears Out Of Sync. → Task 9.
5. The agent has a cached configuration and the server answers with an error, garbage, or an item key the agent does not know: it keeps measuring with what it understands and never ends up measuring nothing. → Tasks 11 and 12.

## File Structure

```
crates/cerbrix-common/src/
  items.rs          built-in item catalogue, default agent items
  interval.rs       Interval: "30s" / "5m" / "1h" / "1d" as a serde type
  config/mod.rs     Severity, name validation, re-exports
  config/host.rs    HostFile, TemplateFile, ItemEntry, TriggerEntry, NotificationFile
  config/dashboard.rs  DashboardFile, Panel, DisplayFile
  trigger.rs        trigger expression parser
  agent_config.rs   ItemConfig, ConfiguredItem (wire types)
crates/cerbrix-common/tests/schemas.rs   schema drift and example validation
schemas/*.schema.json                    generated, committed
examples/config-repo/…                   a valid example repository
crates/cerbrix-server/src/gitops/
  mod.rs            Snapshot, SyncIssue
  tree.rs           read a directory into a Snapshot
  compile.rs        parse, merge templates, cross-check hosts
  boards.rs         validation of dashboards and displays
  git.rs            ConfigSource trait, GitSource
  sync.rs           one sync cycle and the loop
crates/cerbrix-server/src/state/sync.rs  apply, sync state, history, lock
crates/cerbrix-server/migrations/0002_m2.sql
crates/cerbrix-agent/src/
  remote_config.rs  cache on disk, plan(): items → collector groups
  collectors/select.rs  FilteredCollector
docs/configuration-repository.md
```

---

### Task 1: Item catalogue, intervals and host/template/notification file types

**Files:**
- Create: `crates/cerbrix-common/src/items.rs`, `interval.rs`, `agent_config.rs`, `config/mod.rs`, `config/host.rs`
- Modify: `crates/cerbrix-common/src/lib.rs`, `crates/cerbrix-common/Cargo.toml` (add `schemars` with a derive on every type below)

**Interfaces:**
- Produces:

```rust
// items.rs
pub struct ItemSpec { pub key: &'static str, pub collector: &'static str, pub param: Option<&'static str> }
pub const BUILTIN_ITEMS: &[ItemSpec];            // the 17 keys of docs/items.md
pub fn builtin_item(key: &str) -> Option<&'static ItemSpec>;
pub fn default_agent_items() -> Vec<ConfiguredItem>;  // every built-in key, no params

// interval.rs — serialises as the shortest exact form ("90s" stays "90s", 300 s is "5m")
pub struct Interval(Duration);
impl Interval { pub const MIN: Duration = 1 s; pub const MAX: Duration = 24 h; pub fn as_secs(&self) -> u64; }
impl FromStr for Interval;                       // "<digits><s|m|h|d>", nothing else

// agent_config.rs — named ItemConfig because the agent crate already has an AgentConfig (its local file)
pub struct ConfiguredItem { pub key: String, pub params: Option<BTreeMap<String, String>>, pub interval_secs: u64 }
pub struct ItemConfig { pub version: String, pub items: Vec<ConfiguredItem> }

// config/mod.rs
pub enum Severity { Info, Warning, High, Critical }   // serde lowercase, derives Ord in this order
pub fn validate_name(s: &str) -> Result<(), ValidationError>;  // ^[a-z0-9][a-z0-9_-]{0,62}$, for groups and templates

// config/host.rs — all with #[serde(deny_unknown_fields)]
pub struct ItemEntry { pub key: String, pub params: Option<BTreeMap<String, String>>, pub interval: Interval }
pub struct TriggerEntry { pub name: String, pub expression: String, pub severity: Severity }
pub struct HostFile { pub host: String, pub address: Option<String>,
    #[serde(default)] pub groups: Vec<String>, #[serde(default)] pub templates: Vec<String>,
    #[serde(default)] pub items: Vec<ItemEntry>, #[serde(default)] pub triggers: Vec<TriggerEntry> }
pub struct TemplateFile { pub template: String,
    #[serde(default)] pub items: Vec<ItemEntry>, #[serde(default)] pub triggers: Vec<TriggerEntry> }
pub struct NotificationFile { pub name: String, pub email: Vec<String>,
    pub min_severity: Severity, #[serde(default)] pub groups: Vec<String> }
```

`collector` is one of `cpu`, `load`, `memory`, `uptime`, `disk`, `net`; the swap keys belong to `memory`. `param` is `Some("mount")` for `disk.*`, `Some("interface")` for `net.*`. Default intervals in `default_agent_items()`: 30 s for cpu, load, memory and net keys; 300 s for uptime and disk keys.

- [ ] **Step 1: Write the failing tests** (in each file's `tests` module)

  - `catalogue_matches_items_md`: `BUILTIN_ITEMS.len() == 17`; `builtin_item("disk.used_percent").unwrap().param == Some("mount")`; `builtin_item("disk.usedpercent").is_none()`; every key passes `validate_item_key`.
  - `default_items_use_m1_intervals`: `cpu.usage` → 30, `disk.used_percent` → 300, `system.uptime_seconds` → 300.
  - `interval_parses_units`: `"30s"` → 30, `"5m"` → 300, `"1h"` → 3600, `"1d"` → 86400.
  - `interval_rejects`: `"0s"`, `"25h"`, `"2d"`, `"5"`, `"5 m"`, `"1.5h"`, `"-5s"`, `""`, `"5M"`.
  - `interval_round_trips`: `"5m"` and `"90s"` serialise to themselves.
  - `severity_orders_and_parses`: `Info < Warning < High < Critical`; `"high"` parses; `"High"` and `"error"` do not.
  - `host_file_parses_spec_example`: the host file from design section 5.6, verbatim, parses; `items[1].params` is `{"mount": "/"}`.
  - `host_file_rejects_unknown_field`: `{"host":"a","itmes":[]}` fails and the message names `itmes`.
  - `validate_name_rules`: accepts `kunde-a`, `region_2`, `a`; rejects `""`, `Kunde`, `-a`, 64 characters, `a b`.

- [ ] **Step 2: Run** `cargo test -p cerbrix-common` — expected: the new tests fail to compile.
- [ ] **Step 3: Implement the types above.** Derive `Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq`. `Interval` implements `JsonSchema` by hand as a string with pattern `^[0-9]+[smhd]$`.
- [ ] **Step 4: Run** `cargo test -p cerbrix-common` — expected: all pass.
- [ ] **Step 5: Commit** `feat(common): item catalogue, intervals and configuration file types`

---

### Task 2: Trigger expression parser

**Files:**
- Create: `crates/cerbrix-common/src/trigger.rs`
- Modify: `crates/cerbrix-common/src/lib.rs`

**Interfaces:**
- Consumes: `validate_item_key`, `validate_param_name`, `Interval`.
- Produces:

```rust
pub struct Selector { pub key: String, pub params: BTreeMap<String, String> }
pub enum Comparison { Gt, Lt, Ge, Le, Eq, Ne }
pub enum TriggerExpr {
    Compare { selector: Selector, op: Comparison, threshold: f64, hold: Option<Duration> },
    NoData  { selector: Option<Selector>, window: Duration },
}
pub struct TriggerParseError { pub column: usize, pub message: String }   // column is 1-based
pub fn parse_trigger(input: &str) -> Result<TriggerExpr, TriggerParseError>;
```

Grammar (whitespace between tokens is free, keywords are lower case):

```
expr      = compare | nodata
compare   = selector op number [ "for" duration ]
nodata    = [ selector ] "no" "data" "for" duration
selector  = item-key [ "{" param { "," param } "}" ]
param     = name "=" ( bare | quoted )
bare      = one or more characters other than  , } " and whitespace
quoted    = '"' any characters except '"' '"'
op        = ">" | "<" | ">=" | "<=" | "==" | "!="
number    = optional "-", digits, optional "." digits      (must be finite)
duration  = the Interval syntax, 1s to 24h
```

- [ ] **Step 1: Write the failing tests**

  - `parses_spec_example`: `disk.used_percent{mount=/} > 90 for 10m` → `Compare` with key `disk.used_percent`, params `{"mount": "/"}`, `Gt`, `90.0`, hold 600 s.
  - `parses_quoted_param`: `disk.used_percent{mount="/var/lib/my data"} >= 95` → that mount, `Ge`, no hold.
  - `parses_every_operator`: one case per operator.
  - `parses_negative_and_decimal`: `system.load1 > -0.5` → `-0.5`.
  - `parses_host_nodata`: `no data for 3m` → `NoData { selector: None, window: 180 s }`.
  - `parses_item_nodata`: `cpu.usage no data for 5m` → selector key `cpu.usage`.
  - `rejects_with_column`: `cpu.usage >> 90` → column 12; `cpu.usage > ninety` → column 13; `cpu.usage > 90 for` → column at end of input; `cpu.usage{mount=} > 1`; `cpu.usage > 90 for 0s`; `cpu.usage > 90 extra`; `""`; `cpu.usage > NaN`; `cpu.usage > 1e400`.
  - `rejects_invalid_key_and_param_name`: `CPU.usage > 1`; `cpu.usage{1x=a} > 1`.

- [ ] **Step 2: Run** `cargo test -p cerbrix-common trigger` — expected: fail to compile.
- [ ] **Step 3: Implement `parse_trigger`** as a hand-written recursive-descent parser over a `char_indices` cursor; no parser dependency.
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(common): trigger expression parser`

---

### Task 3: Dashboard and display file types

**Files:**
- Create: `crates/cerbrix-common/src/config/dashboard.rs`
- Modify: `crates/cerbrix-common/src/config/mod.rs`

**Interfaces:**
- Produces (all `deny_unknown_fields`, all derive `JsonSchema`):

```rust
pub const GRID_COLUMNS: u8 = 12;
pub const GRID_ROWS: u8 = 6;

#[serde(default)] pub struct HostSelector { pub groups: Vec<String>, pub names: Vec<String> }
impl HostSelector { pub fn selects(&self, host: &str, host_groups: &[String]) -> bool; }
// groups: host must be in ALL of them. names: additional single hosts. Both empty: every host.

pub struct Placement { pub x: u8, pub y: u8, pub w: u8, pub h: u8 }
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Panel {
    Problems   { #[serde(flatten)] at: Placement },
    HostStates { #[serde(flatten)] at: Placement },
    Graph { host: String, item: String, params: Option<BTreeMap<String, String>>,
            #[serde(default = "six_hours")] range: Interval, #[serde(flatten)] at: Placement },
    Value { host: String, item: String, params: Option<BTreeMap<String, String>>,
            #[serde(flatten)] at: Placement },
}
impl Panel { pub fn placement(&self) -> Placement; }
pub struct DashboardFile { pub name: String, #[serde(default)] pub priority: i32,
    #[serde(default)] pub hosts: HostSelector, pub panels: Option<Vec<Panel>> }
impl DashboardFile { pub fn effective_panels(&self) -> Vec<Panel>; }
// None → Problems at 0,0 8×6 and HostStates at 8,0 4×6.

#[serde(untagged)]
pub enum Show { Fixed { dashboard: String }, Rotate { rotate: Vec<String>, every: Interval } }
pub struct SwitchOnAlarm { #[serde(default = "critical")] pub min_severity: Severity }
pub struct DisplayFile { pub name: String, pub show: Show,
    pub switch_on_alarm: Option<SwitchOnAlarm>, pub dashboards: Option<Vec<String>> }
```

`deny_unknown_fields` does not combine with `flatten`; for `Panel` enforce unknown fields by a hand-written `Deserialize` that reads a `serde_json::Map`, removes the known keys per type and fails on any that remain.

- [ ] **Step 1: Write the failing tests**

  - `dashboard_parses_concept_example`: the dashboard JSON from UI concept section 5.1, verbatim → 4 panels; the graph panel has `range` 6 h and placement `8,3 4×2`.
  - `minimal_dashboard_gets_standard_layout`: `{"name":"A","hosts":{"groups":["kunde-a"]}}` → `priority == 0`, `effective_panels()` is the two standard panels.
  - `panel_rejects_unknown_type_and_field`: `{"type":"pie",…}` fails; `{"type":"problems","x":0,"y":0,"w":1,"h":1,"colour":"red"}` fails naming `colour`.
  - `selector_requires_all_groups`: groups `["kunde-a","region-2"]` selects a host in both, not a host in one; `names: ["db-02"]` selects `db-02` regardless; the empty selector selects everything.
  - `display_parses_fixed_and_rotate`: the display JSON from concept section 5.2 → `Rotate` with 3 ids and 30 s, `min_severity == Critical`; `{"name":"x","show":{"dashboard":"a"}}` → `Fixed`, no switching.
  - `display_rejects_ambiguous_show`: `{"dashboard":"a","rotate":["b"],"every":"30s"}` fails.

- [ ] **Step 2: Run** `cargo test -p cerbrix-common dashboard` — expected: fail to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(common): dashboard and display file types`

---

### Task 4: Published JSON Schemas and the example repository

**Files:**
- Create: `schemas/host.schema.json`, `template.schema.json`, `notification.schema.json`, `dashboard.schema.json`, `display.schema.json`
- Create: `examples/config-repo/hosts/web-01.json`, `hosts/db/db-01.json`, `templates/linux-base.json`, `notifications/ops.json`, `dashboards/production.json`, `displays/noc.json`, `README.md`
- Create: `crates/cerbrix-common/tests/schemas.rs`; add `jsonschema` as a dev-dependency

**Interfaces:**
- Consumes: the file types of Tasks 1 and 3.
- Produces: `pub fn schema_json<T: JsonSchema>() -> String` in `cerbrix_common::config` (pretty-printed, trailing newline, with `$id` `https://cerbrix.dev/schemas/<name>.schema.json`); the committed schema files; an example repository that later tasks use as their valid fixture.

The example repository: `linux-base` has `cpu.usage` 30s, `memory.used_percent` 30s, `disk.used_percent` 5m and the triggers "Disk almost full" (`disk.used_percent > 90 for 10m`, high) and "Agent silent" (`no data for 3m`, critical). `web-01` is in groups `production`, `web`, uses `linux-base`, overrides `cpu.usage` to 10s. `db-01` is in `production`, `db`, uses `linux-base` and adds `disk.used_percent{mount=/}` at 1m. The dashboard selects group `production` and has a graph of `db-01` `disk.used_percent`. The display shows that dashboard and switches on `critical`.

- [ ] **Step 1: Write the failing tests** in `tests/schemas.rs`

  - `committed_schemas_are_current`: for each of the five types, `schema_json::<T>()` equals the file under `schemas/` byte for byte. The failure message says: run `UPDATE_SCHEMAS=1 cargo test -p cerbrix-common --test schemas`.
  - `example_repo_validates_against_schemas`: every JSON file under `examples/config-repo/` validates against the schema of its root directory.
  - `schema_rejects_what_serde_rejects`: `{"host":"a","itmes":[]}` fails the host schema; a panel with `"type":"pie"` fails the dashboard schema; severity `"error"` fails the host schema.

- [ ] **Step 2: Run** `cargo test -p cerbrix-common --test schemas` — expected: fail (no schema files).
- [ ] **Step 3: Implement `schema_json`, write the example repository, and generate the schema files** with `UPDATE_SCHEMAS=1` (the test writes the files when that variable is set).
- [ ] **Step 4: Run** the tests without the variable — expected: pass.
- [ ] **Step 5: Commit** `feat: published JSON Schemas and example configuration repository`

---

### Task 5: Compile a snapshot: parsing, template merge, cross-checks

**Files:**
- Create: `crates/cerbrix-server/src/gitops/mod.rs`, `compile.rs`, `boards.rs`
- Modify: `crates/cerbrix-server/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces:

```rust
// gitops/mod.rs
pub struct Snapshot { pub commit: String, pub files: BTreeMap<String, Vec<u8>> }  // keys like "hosts/db/db-01.json"
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SyncIssue { pub file: Option<String>, pub line: Option<u32>, pub column: Option<u32>, pub reason: String }
pub const MAX_ISSUES: usize = 50;

// gitops/compile.rs
#[derive(Serialize, Deserialize)] pub struct EffectiveItem { pub key: String,
    pub params: Option<BTreeMap<String, String>>, pub interval_secs: u64, pub origin: String }
#[derive(Serialize, Deserialize)] pub struct EffectiveTrigger { pub name: String,
    pub expression: String, pub severity: Severity, pub origin: String }
#[derive(Serialize, Deserialize)] pub struct EffectiveHost { pub templates: Vec<String>,
    pub items: Vec<EffectiveItem>, pub triggers: Vec<EffectiveTrigger> }
impl EffectiveHost { pub fn agent_config(&self) -> ItemConfig; }
pub struct CompiledHost { pub name: String, pub address: Option<String>, pub groups: Vec<String>,
    pub source_file: String, pub effective: EffectiveHost }
pub enum DocumentKind { Template, Notification, Dashboard, Display }   // as_str(): "template" …
pub struct Document { pub kind: DocumentKind, pub id: String, pub source_file: String, pub body: serde_json::Value }
pub struct Compiled { pub hosts: Vec<CompiledHost>, pub documents: Vec<Document> }
pub fn compile(snapshot: &Snapshot) -> Result<Compiled, Vec<SyncIssue>>;
```

Rules `compile` enforces (every violation is one `SyncIssue` with the file; all issues are collected, sorted by file, cut at `MAX_ISSUES`):

- Only paths matching `^(hosts|templates|notifications|dashboards|displays)/.+\.json$` are read; everything else is ignored.
- A file that is not valid UTF-8, starts with a BOM, or does not parse: issue with `line` and `column` from `serde_json` where it gives them.
- `origin` is `"host"` or `"template:<name>"`.
- **Merge:** an item is identified by `(key, params)`, a trigger by `name`. Start empty, apply the host's templates in listed order, then the host's own entries; a later entry replaces an earlier one with the same identity. Output order: sorted by key then params for items, by name for triggers.
- Host names pass `validate_host_name` and are unique; template names and group names pass `validate_name`; template names are unique; a host's `templates` exist.
- Item keys exist in `BUILTIN_ITEMS`; `params` may only contain that item's parameter name; items without a parameter take no `params`.
- Every trigger expression parses (a parse error gives the column inside the expression in the reason); its selector key is one of the host's effective item keys, and its params are allowed for that item.
- Notifications: unique `name`, at least one address, each address contains exactly one `@` with text on both sides, group names valid.
- Dashboards and displays (`boards.rs`): id is the file stem and passes `validate_name`; ids and `name`s unique per kind; every placement has `w ≥ 1`, `h ≥ 1`, `x + w ≤ 12`, `y + h ≤ 6`; no two panels overlap; a `graph` or `value` panel's host exists, is selected by the dashboard, and has the item key in its effective items; a display's `show` targets and `dashboards` entries exist; with `dashboards` set, every `show` target is in it; `rotate` is not empty; `every` is between 5 s and 1 h.
- `agent_config()`: the effective items as `ConfiguredItem`s; `version` is the lower-case hex SHA-256 of their JSON serialisation.

- [ ] **Step 1: Write the failing tests** (a helper `snap(&[(path, json)])` builds a snapshot; a helper loads `examples/config-repo`)

  - `example_repo_compiles`: 2 hosts, 4 documents.
  - `host_entry_beats_template`: `web-01` has `cpu.usage` at 10 s with origin `host`, `memory.used_percent` at 30 s with origin `template:linux-base`.
  - `later_template_wins`: templates `a` (cpu 30s) and `b` (cpu 60s), host lists `["a","b"]` → 60 with origin `template:b`; listed `["b","a"]` → 30.
  - `specific_and_general_item_coexist`: `db-01` has both `disk.used_percent` (300 s) and `disk.used_percent{mount=/}` (60 s).
  - `trigger_override_by_name`: a host trigger named "Disk almost full" replaces the template's.
  - `syntax_error_has_position`: a host file with a missing comma on line 3 → one issue, `file == "hosts/a.json"`, `line == Some(3)`, `column.is_some()`.
  - `unknown_item_key`: `disk.usedpercent` → reason contains `unknown item key "disk.usedpercent"`.
  - `wrong_param_for_item`: `cpu.usage` with `params {"mount":"/"}`; `disk.used_percent` with `{"interface":"eth0"}`.
  - `unknown_template`, `duplicate_host_name` (issue names both files), `duplicate_template_name`.
  - `trigger_on_item_the_host_lacks`: template trigger on `net.rx_bytes_per_sec`, host without that item → issue at the host file naming the trigger.
  - `invalid_trigger_expression`: reason contains `column 12`.
  - `all_issues_are_collected`: three broken files → three issues, sorted by file; 60 broken files → 50 issues.
  - `ignores_other_files`: `README.md`, `hosts/notes.txt`, `scripts/x.json` change nothing.
  - `empty_snapshot_is_valid`: no files → `Compiled` with nothing in it.
  - `bom_and_invalid_utf8_rejected`.
  - Boards: `overlapping_panels`, `panel_outside_grid` (x=10,w=3 and y=5,h=2), `zero_sized_panel`, `graph_host_not_selected`, `graph_item_not_on_host`, `duplicate_dashboard_stem_in_subdirectories` (`dashboards/a/x.json` and `dashboards/b/x.json`), `display_unknown_dashboard`, `display_show_outside_allowed_list`, `display_empty_rotation`, `display_rotation_too_fast` (`"1s"`).
  - `agent_config_version_changes_with_items`: equal items → equal version; a changed interval → a different version; 64 hex characters.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server gitops` — expected: fail to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): compile and validate a configuration snapshot`

---

### Task 6: Read a directory tree and the `check-config` command

**Files:**
- Create: `crates/cerbrix-server/src/gitops/tree.rs`
- Modify: `crates/cerbrix-server/src/gitops/mod.rs`, `crates/cerbrix-server/src/main.rs`

**Interfaces:**
- Consumes: `Snapshot`, `SyncIssue`, `compile`.
- Produces:

```rust
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_FILES: usize = 20_000;
pub fn read_tree(root: &Path, commit: &str) -> Result<Snapshot, Vec<SyncIssue>>;
pub fn format_issue(issue: &SyncIssue) -> String;
// "hosts/a.json:3:7: expected `,`" | "hosts/a.json: unknown template \"x\"" | "repository unreachable"
```

`read_tree` walks only the five root directories, never follows symbolic links, and reports as issues: a symbolic link (file or directory), an entry that is neither file nor directory, a `.json` file larger than `MAX_FILE_BYTES`, more than `MAX_FILES` files. Path separators in keys are `/` on every platform. A missing root directory is not an error.

CLI: `cerbrix-server check-config <DIR>` runs `read_tree` and `compile` and needs neither a configuration file nor a database, so `main` handles it before `Config::load`. Exit 0 and `configuration is valid: 2 hosts, 1 template, 1 notification, 1 dashboard, 1 display` on stdout; exit 1 and one `format_issue` line per issue on stderr.

- [ ] **Step 1: Write the failing tests**

  - `reads_nested_files_with_forward_slashes`: `hosts/db/db-01.json` is a key.
  - `ignores_files_outside_roots_and_non_json`.
  - `rejects_symlink_file` and `rejects_symlink_directory` (`#[cfg(unix)]`): the issue names the path and says `symbolic links are not allowed`; a link to a file outside the root is never opened (point it at a path that does not exist and assert the reason is the symlink reason, not a read error).
  - `rejects_oversized_file`: a 1 MiB + 1 byte `.json`.
  - `missing_roots_give_empty_snapshot`.
  - `format_issue_variants`: the three shapes above.
  - Integration (`tests/check_config.rs`, runs the built binary via `env!("CARGO_BIN_EXE_cerbrix-server")`): the example repository → exit 0 and the summary line; a copy with a broken host file → exit 1, stderr has `hosts/web-01.json:` and no summary; works with `CERBRIX_CONFIG` unset and no database reachable.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server tree check_config` — expected: fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): read configuration trees and add check-config`

---

### Task 7: State store: apply a revision, sync state, history

**Files:**
- Create: `crates/cerbrix-server/migrations/0002_m2.sql`, `crates/cerbrix-server/src/state/sync.rs`
- Modify: `crates/cerbrix-server/src/state/mod.rs`, `state/hosts.rs`

**Interfaces:**
- Consumes: `Compiled`, `CompiledHost`, `Document`, `EffectiveHost`, `SyncIssue`.
- Produces:

```rust
pub enum SyncStatus { Never, InSync, OutOfSync }
pub struct SyncState { pub status: SyncStatus, pub active_commit: Option<String>,
    pub last_success_at: Option<OffsetDateTime>, pub last_attempt_at: Option<OffsetDateTime>,
    pub attempted_commit: Option<String>, pub issues: Vec<SyncIssue> }
#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
pub struct ChangeSet { pub added: Vec<String>, pub changed: Vec<String>, pub removed: Vec<String> }
#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
pub struct SyncChanges { pub hosts: ChangeSet, pub templates: ChangeSet, pub notifications: ChangeSet,
    pub dashboards: ChangeSet, pub displays: ChangeSet }
pub struct SyncHistoryEntry { pub commit: String, pub applied_at: OffsetDateTime, pub changes: SyncChanges }
pub enum ApplyError { MassRemoval { removed: usize, active: usize }, State(StateError) }
pub struct SyncLock { /* holds one pooled connection */ }

impl StateStore {
    pub async fn apply_config(&self, commit: &str, compiled: &Compiled, now: OffsetDateTime,
        max_host_removal_percent: u8) -> Result<SyncChanges, ApplyError>;
    pub async fn record_sync_failure(&self, attempted_commit: Option<&str>, issues: &[SyncIssue],
        now: OffsetDateTime) -> Result<(), StateError>;
    pub async fn record_sync_unchanged(&self, now: OffsetDateTime) -> Result<(), StateError>;
    pub async fn sync_state(&self) -> Result<SyncState, StateError>;
    pub async fn sync_history(&self, limit: i64) -> Result<Vec<SyncHistoryEntry>, StateError>;
    pub async fn host_config(&self, host_id: Uuid) -> Result<Option<EffectiveHost>, StateError>;
    pub async fn try_sync_lock(&self) -> Result<Option<SyncLock>, StateError>;
}
impl SyncLock { pub async fn release(self) -> Result<(), StateError>; }
```

Migration `0002_m2.sql`:

```sql
ALTER TABLE hosts
    ADD COLUMN address     text,
    ADD COLUMN groups      text[] NOT NULL DEFAULT '{}',
    ADD COLUMN source_file text,
    ADD COLUMN config      jsonb;

CREATE TABLE config_documents (
    kind        text NOT NULL CHECK (kind IN ('template', 'notification', 'dashboard', 'display')),
    id          text NOT NULL,
    source_file text NOT NULL,
    body        jsonb NOT NULL,
    PRIMARY KEY (kind, id)
);

CREATE TABLE sync_state (
    singleton        boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    status           text NOT NULL CHECK (status IN ('never', 'in_sync', 'out_of_sync')),
    active_commit    text,
    last_success_at  timestamptz,
    last_attempt_at  timestamptz,
    attempted_commit text,
    issues           jsonb NOT NULL DEFAULT '[]'
);
INSERT INTO sync_state (status) VALUES ('never');

CREATE TABLE sync_history (
    id         bigserial PRIMARY KEY,
    commit     text NOT NULL,
    applied_at timestamptz NOT NULL,
    changes    jsonb NOT NULL
);
```

`apply_config`, in one transaction:
- Hosts in `compiled`: insert, or update `address`, `groups`, `source_file`, `config`, and set `active = true`. "Changed" means any of those four differ or the host was inactive.
- Active hosts not in `compiled`: `active = false`, `config = NULL`, and their row in `agent_credentials` is deleted. They are "removed". Host rows are never deleted.
- Guard before any write: with `active ≥ 10` and `removed * 100 > active * max_host_removal_percent` return `MassRemoval` and change nothing.
- `config_documents` is replaced; the change sets compare `body` and `source_file`.
- `sync_state` becomes `in_sync`, `active_commit = commit`, `last_success_at = last_attempt_at = now`, `attempted_commit = commit`, `issues = []`.
- One `sync_history` row, also when nothing changed in content but the commit is new.

`record_sync_failure` sets `out_of_sync`, `last_attempt_at`, `attempted_commit`, `issues`, and leaves `active_commit`, `last_success_at` and all configuration untouched. `record_sync_unchanged` sets `in_sync`, both timestamps, clears `issues`, writes no history. `try_sync_lock` takes a connection from the pool and calls `pg_try_advisory_lock(0x43425258_53594E43)`; `release` unlocks and returns the connection. `create_host` (used by `admin host add`) keeps writing `config = NULL`.

- [ ] **Step 1: Write the failing tests** (each on a fresh schema, as the M1 state tests do)

  - `first_apply_creates_everything`: the example repository → 2 hosts added, state `InSync` with the commit, history has 1 entry whose `changes.hosts.added == ["db-01","web-01"]`, `host_config` returns the effective host.
  - `second_apply_reports_changes`: change `web-01`'s interval and drop the dashboard → `hosts.changed == ["web-01"]`, `dashboards.removed == ["production"]`, nothing added.
  - `removed_host_is_deactivated_and_credential_revoked`: enrol `db-01`, apply without it → `host_by_name("db-01").active == false`, `host_for_credential(old)` is `None`, `host_config` is `None`.
  - `readded_host_is_active_without_credential`: apply with `db-01` again → active, in `hosts.changed`, the old credential still unknown, `bind_credential` succeeds.
  - `failure_keeps_active_configuration`: apply, then `record_sync_failure(Some("bad"), [issue])` → status `OutOfSync`, `active_commit` unchanged, `attempted_commit == "bad"`, issues round-trip, hosts untouched, history length unchanged.
  - `unchanged_clears_out_of_sync`: after a failure, `record_sync_unchanged` → `InSync`, no issues, history length unchanged.
  - `mass_removal_is_refused`: 10 active hosts, apply a revision with 4 at percent 50 → `MassRemoval { removed: 6, active: 10 }`, all 10 still active with credentials; with percent 100 it applies; with 9 active hosts it applies.
  - `apply_is_atomic`: a hand-built `Compiled` with one new host and two documents that share `(kind, id)` (which `compile` never produces) → the insert into `config_documents` fails; `apply_config` returns `State`, the new host does not exist, and `sync_state` and the history are unchanged.
  - `sync_lock_is_exclusive`: a second `try_sync_lock` returns `None` until the first is released.
  - `history_is_newest_first_and_limited`.
  - `manual_hosts_are_removed_by_first_sync`: a host from `create_host` that is not in the revision is deactivated.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server state::sync` — expected: fail to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): store applied configuration, sync state and history`

---

### Task 8: GitOps settings and the Git source

**Files:**
- Create: `crates/cerbrix-server/src/gitops/git.rs`
- Modify: `crates/cerbrix-server/src/config.rs`, `crates/cerbrix-server/src/gitops/mod.rs`

**Interfaces:**
- Consumes: `Snapshot`, `SyncIssue`, `read_tree`.
- Produces:

```rust
// config.rs — Config gains `#[serde(default)] pub gitops: GitopsConfig`
pub struct GitopsConfig {
    pub enabled: bool,                       // default false
    pub url: Option<String>,
    pub branch: String,                      // default "main"
    pub interval_secs: u64,                  // default 60, allowed 5..=3600
    pub token: Option<String>,
    pub ssh_key_file: Option<PathBuf>,
    pub known_hosts_file: Option<PathBuf>,
    pub cache_dir: PathBuf,                  // default "/var/lib/cerbrix/git"
    pub max_host_removal_percent: u8,        // default 50, allowed 1..=100
}

// gitops/git.rs
pub enum SourceError { Fetch(String), Invalid(Vec<SyncIssue>) }
#[async_trait] pub trait ConfigSource: Send + Sync {
    async fn fetch(&self) -> Result<Snapshot, SourceError>;
}
pub struct GitSource { /* url, branch, auth, cache_dir */ }
impl GitSource { pub fn new(cfg: &GitopsConfig) -> Self; }
pub(crate) fn scrub(text: &str, secrets: &[&str]) -> String;
```

Config validation: with `enabled`, `url` is required; `token` and `ssh_key_file` are mutually exclusive; `token` requires an `https://` URL; `ssh_key_file` requires an `ssh://` or `user@host:path` URL; a URL with a password in it is rejected with "put the token in gitops.token". `Debug` for `GitopsConfig` prints `token: <redacted>` and the URL without userinfo. Environment names follow the existing scheme (`CERBRIX_GITOPS__TOKEN`).

`GitSource::fetch`, each `git` call via `tokio::process::Command` with `kill_on_drop`, a 60 s timeout, `GIT_TERMINAL_PROMPT=0`, and the configuration passed through `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n` so nothing secret is on a command line:
1. If `cache_dir/repo` is missing: `git init`, `git remote add origin <url>`. If the remote URL differs from the configured one: `git remote set-url`.
2. `git fetch --depth 1 --no-tags origin <branch>`.
3. `git checkout --force --detach FETCH_HEAD`, then `git clean -fdx`.
4. `git rev-parse HEAD` → commit; `read_tree(cache_dir/repo, commit)`.

Authentication: token → `http.extraHeader = Authorization: Basic base64("x-access-token:<token>")`. SSH key → `core.sshCommand = ssh -i <key> -o IdentitiesOnly=yes -o BatchMode=yes -o UserKnownHostsFile=<file> -o StrictHostKeyChecking=<yes|accept-new>`, where the file is `known_hosts_file` with `yes`, or `cache_dir/known_hosts` with `accept-new`. Always set `core.symlinks=false`, `protocol.file.allow` only as needed for the tests' local paths, and `credential.helper=` (empty).

A failing `git` call → `SourceError::Fetch(reason)`, where reason is the first non-empty line of stderr after `scrub` (replaces the token, its base64 form and any `//user:pass@` userinfo by `***`), cut to 300 characters. A missing `git` program → `Fetch("the git program was not found")`. Issues from `read_tree` → `SourceError::Invalid`.

- [ ] **Step 1: Write the failing tests**

  - Config: `gitops_defaults` (disabled, branch `main`, 60 s, percent 50); `enabled_requires_url`; `token_and_key_are_exclusive`; `token_requires_https`; `url_with_password_rejected`; `interval_bounds` (4 and 3601 rejected); `debug_redacts_token`; `token_from_environment`.
  - `scrub_removes_secrets`: `fatal: unable to access 'https://x:tok123@host/r.git/'` with secret `tok123` → no `tok123`, contains `***`.
  - Git (a helper creates a repository in a temp dir with `git init -b main`, commits the example repository with a fixed author via `-c user.name=… -c user.email=…`; the source's URL is the directory path):
    - `fetches_head_of_branch`: `commit` equals `git rev-parse HEAD` of the origin; `files` has `hosts/web-01.json`.
    - `second_fetch_sees_new_commit`: commit a change, fetch → new commit, new content; a file deleted in the origin is gone from `files`.
    - `unknown_branch_is_fetch_error`: branch `nope` → `Fetch`, reason mentions `nope`.
    - `unreachable_repository_is_fetch_error`: a path that does not exist.
    - `symlink_in_repository_is_invalid` (`#[cfg(unix)]`): `Invalid` with the symlink reason.
    - `cache_survives_url_change`: point a second source with the same cache at another origin → its content.
  - Authentication (no network; assert on a `pub(crate) fn git_config(cfg: &GitopsConfig) -> Vec<(String, String)>` that builds the key/value pairs):
    - `token_becomes_basic_auth_header`: token `abc` → `http.extraHeader` is `Authorization: Basic ` + base64 of `x-access-token:abc`; no `core.sshCommand`.
    - `ssh_key_without_known_hosts_trusts_on_first_use`: `core.sshCommand` contains `-i <key>`, `IdentitiesOnly=yes`, `BatchMode=yes`, `StrictHostKeyChecking=accept-new` and the cache's `known_hosts`.
    - `ssh_key_with_known_hosts_is_strict`: `StrictHostKeyChecking=yes` and the given file.
    - `always_disables_symlinks_and_helpers`: `core.symlinks=false` and an empty `credential.helper` in every case.

  Fetching over real HTTPS with a token and over real SSH is not covered by automated tests, because it needs real credentials. It is a manual check for the project owner before M2 is called done; report it as unverified.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server config gitops::git` — expected: fail to compile.
- [ ] **Step 3: Implement.** Add `base64` (already a dependency) and tokio's `process` feature.
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): GitOps settings and fetching the repository with git`

---

### Task 9: The sync cycle, the loop and the admin commands

**Files:**
- Create: `crates/cerbrix-server/src/gitops/sync.rs`
- Modify: `crates/cerbrix-server/src/tls.rs` (start the loop in `serve`), `crates/cerbrix-server/src/admin.rs`, `crates/cerbrix-server/src/main.rs`

**Interfaces:**
- Consumes: `ConfigSource`, `compile`, the `StateStore` methods of Task 7, `GitopsConfig`, `format_issue`.
- Produces:

```rust
pub enum SyncOutcome { Applied(SyncChanges), Unchanged, OutOfSync(Vec<SyncIssue>), Skipped }
pub async fn sync_once(store: &StateStore, source: &dyn ConfigSource,
    max_host_removal_percent: u8, now: OffsetDateTime) -> Result<SyncOutcome, StateError>;
pub async fn run_sync_loop(store: StateStore, source: Arc<dyn ConfigSource>,
    cfg: GitopsConfig, shutdown: CancellationToken);
```

`sync_once`:
1. `try_sync_lock`; `None` → `Skipped`.
2. `source.fetch()`. `Fetch(reason)` → `record_sync_failure(None, [issue without file])` → `OutOfSync`. `Invalid(issues)` → `record_sync_failure(None, issues)`.
3. If the commit equals `active_commit`: `record_sync_unchanged` → `Unchanged`.
4. `compile`. Issues → `record_sync_failure(Some(commit), issues)` → `OutOfSync`.
5. `apply_config`. `MassRemoval { removed, active }` → failure with reason `this revision removes {removed} of {active} active hosts; raise gitops.max_host_removal_percent to apply it`.
6. Release the lock on every path.

`run_sync_loop` calls `sync_once` immediately and then every `interval_secs`, until shutdown. It logs one `info` line per `Applied` (commit and counts), one `warn` line when the state changes to Out Of Sync or its issues change, and nothing for `Unchanged`. A `StateError` is logged and the loop continues. `serve` spawns it only when `gitops.enabled`, and awaits it on shutdown.

Admin:
- `admin host add` with GitOps on fails: `hosts are managed by Git; add a file under hosts/ in the repository`. `host release`, `key …` and `unknown-agents` work in both modes.
- `admin host show <NAME>`: name, active, address, groups, source file, then the effective configuration as pretty JSON. Without stored configuration: `no configuration from Git; the agent uses the built-in defaults`.
- `admin sync status`: `in sync` / `out of sync` / `never synced`, active commit, last successful sync, last attempt, then one `format_issue` line per issue.
- `admin sync history [--limit N]` (default 20): one line per entry: time, first 12 characters of the commit, and counts such as `hosts +2 ~1 -0, dashboards +1`.
- `admin::run` gains a `gitops_enabled: bool` parameter.

- [ ] **Step 1: Write the failing tests** (a `FakeSource` returns queued results)

  - `valid_revision_is_applied`: `Applied`; state `InSync`.
  - `invalid_revision_changes_nothing`: apply A; B has a broken host file → `OutOfSync`; `active_commit == A`, hosts as in A, issue names the file, `attempted_commit == B`.
  - `repeated_invalid_commit_adds_no_history`: three cycles on B → history length 1, state stays Out Of Sync with the same issues.
  - `fixed_revision_clears_out_of_sync`: then C valid → `Applied`, no issues.
  - `fetch_failure_then_same_commit_clears`: apply A; `Fetch("unreachable")` → Out Of Sync with a file-less issue and `active_commit == A`; fetch A again → `Unchanged`, state `InSync`.
  - `mass_removal_reports_reason`: the reason text above with the numbers.
  - `skipped_while_locked`: hold the lock → `Skipped`, and the source was not asked.
  - `lock_is_released_after_failure`: after an `OutOfSync` outcome `try_sync_lock` succeeds.
  - `loop_stops_on_shutdown`: with a 5 s interval the loop returns within 1 s of cancellation and has synced once.
  - Admin: `host_add_refused_with_gitops`; `host_show_prints_effective_configuration` (contains `"origin": "template:linux-base"`); `sync_status_out_of_sync_lists_issue`; `sync_history_formats_counts`.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server gitops::sync admin` — expected: fail to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): Git sync loop with Out Of Sync state and admin commands`

---

### Task 10: Agent configuration endpoint

**Files:**
- Modify: `crates/cerbrix-server/src/api/mod.rs`, `crates/cerbrix-server/src/api/agent.rs`

**Interfaces:**
- Consumes: `AuthedHost`, `StateStore::host_config`, `EffectiveHost::agent_config`, `default_agent_items`.
- Produces: `GET /api/agent/v1/config`, bearer credential as for `/values`.
  - `200` with `ItemConfig` as JSON and the header `ETag: "<version>"`.
  - `304` without a body when `If-None-Match` equals that ETag.
  - A host without stored configuration gets `default_agent_items()` with the version computed the same way.
  - `401` with code `unauthorized` exactly as `/values`; a deactivated host has no credential and therefore gets `401`.

- [ ] **Step 1: Write the failing tests** (router tests as in M1)

  - `config_requires_credential`: no header → 401 `unauthorized`.
  - `config_returns_effective_items`: apply the example repository, enrol `web-01` → 200; items contain `cpu.usage` with `interval_secs` 10; `ETag` is the quoted `version`.
  - `config_not_modified`: repeat with `If-None-Match` → 304, empty body; with another value → 200.
  - `config_version_changes_after_sync`: apply a revision with another interval → a different ETag.
  - `manual_host_gets_defaults`: `create_host` + enrol → 17 items.
  - `removed_host_is_unauthorized`: apply a revision without the host → 401.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server api` — expected: the new tests fail with 404.
- [ ] **Step 3: Implement** `agent::config` and the route.
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): agents fetch their configuration`

---

### Task 11: Agent: fetch, cache and plan the configuration

**Files:**
- Create: `crates/cerbrix-agent/src/remote_config.rs`, `crates/cerbrix-agent/src/collectors/select.rs`
- Modify: `crates/cerbrix-agent/src/client.rs`, `crates/cerbrix-agent/src/collectors/mod.rs`, `crates/cerbrix-agent/src/lib.rs`

**Interfaces:**
- Consumes: `ItemConfig`, `ConfiguredItem`, `builtin_item`, `Collector`, `Reading`.
- Produces:

```rust
// client.rs
impl ServerClient {
    /// Ok(None) = not modified.
    pub async fn fetch_config(&self, credential: &str, known_version: Option<&str>)
        -> Result<Option<ItemConfig>, ClientError>;
}

// remote_config.rs
pub struct ConfigCache { /* state_dir/config.json */ }
impl ConfigCache {
    pub fn new(state_dir: &Path) -> Self;
    pub fn load(&self) -> Option<ItemConfig>;            // unreadable or unparseable → None, logged once
    pub fn save(&self, config: &ItemConfig) -> std::io::Result<()>;   // write temp file, rename
}
pub enum CollectorKind { Cpu, Load, Memory, Uptime, Disk, Net }
pub struct Wanted { pub key: String, pub params: Option<BTreeMap<String, String>>,
                    pub except: Vec<BTreeMap<String, String>> }
pub struct Group { pub collector: CollectorKind, pub interval: Duration, pub wanted: Vec<Wanted> }
pub fn plan(items: &[ConfiguredItem]) -> Vec<Group>;

// collectors/select.rs
pub struct FilteredCollector { /* inner: Box<dyn Collector>, wanted: Vec<Wanted> */ }
impl FilteredCollector { pub fn new(inner: Box<dyn Collector>, wanted: Vec<Wanted>) -> Self; }
impl Collector for FilteredCollector { /* name() is the inner name */ }
```

`plan`: items with a key the agent does not know (`builtin_item` is `None`) or an interval of 0 are skipped with one warning each. The rest are grouped by `(collector, interval)`; groups are sorted by collector then interval. An item without `params` gets, as `except`, the `params` of every item with the same key that has them, so one instance is never reported by two groups. Duplicate `(key, params)` entries: the first wins.

`FilteredCollector::collect` returns the inner readings for which some `Wanted` has the same key and either `params == Some(reading.params)`, or `params == None` and the reading's params are not in `except`.

`fetch_config` sends `If-None-Match: "<known_version>"` when given; 304 → `Ok(None)`; other statuses through the existing `check_status`; a 200 body that does not parse → `ClientError::Retryable`.

- [ ] **Step 1: Write the failing tests**

  - `plan_groups_by_collector_and_interval`: `cpu.usage` 30, `memory.used_percent` 30, `memory.total_bytes` 3600 → three groups: Cpu/30, Memory/30 with one key, Memory/3600 with one key.
  - `plan_specific_instance_excluded_from_general`: `disk.used_percent` 300 and `disk.used_percent{mount=/}` 60 → Disk/60 wants `{mount=/}`; Disk/300 wants the key with `except == [{mount=/}]`.
  - `plan_skips_unknown_key_and_zero_interval`: `gpu.usage` and an interval of 0 are dropped, the known item stays.
  - `plan_of_nothing_is_empty`.
  - `filter_keeps_only_wanted_keys`: an inner collector yielding `memory.used_percent` and `memory.total_bytes`, wanted only the first.
  - `filter_applies_params_and_except`: inner yields mounts `/`, `/data`; wanted general with `except [{mount=/}]` → only `/data`; wanted `{mount=/}` → only `/`.
  - `cache_round_trips` and `cache_ignores_garbage` (a file with `{` → `None`); `cache_save_is_atomic` (no temp file left; an existing cache stays readable if `save` targets a directory that vanished → error, old file not truncated).
  - Client (wiremock): `fetch_config_ok` (asserts the bearer header); `fetch_config_not_modified` (asserts `If-None-Match: "v1"` was sent, returns `Ok(None)`); `fetch_config_unauthorized` → `Unauthorized`; `fetch_config_garbage_is_retryable`; `fetch_config_500_is_retryable`.

- [ ] **Step 2: Run** `cargo test -p cerbrix-agent remote_config select client` — expected: fail to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(agent): fetch, cache and plan the server-side configuration`

---

### Task 12: Agent: run from the fetched configuration and reconfigure live

**Files:**
- Modify: `crates/cerbrix-agent/src/agent.rs`, `crates/cerbrix-agent/src/config.rs`, `crates/cerbrix-agent/src/sender.rs` (expose the current credential), `deploy/cerbrix-agent.service` only if paths change (they do not)

**Interfaces:**
- Consumes: Task 11, `Scheduler`, `CredentialStore`.
- Produces:

```rust
pub const CONFIG_POLL: Duration = Duration::from_secs(60);
pub fn build_scheduler(config: &AgentConfig /* the local file */, groups: &[Group],
    tx: mpsc::Sender<MetricValue>) -> anyhow::Result<Scheduler>;
pub async fn run_config_poller(client: ServerClient, credential: CredentialStore, cache: ConfigCache,
    current: watch::Sender<Option<ItemConfig>>, poll: Duration, shutdown: CancellationToken);
```

Behaviour:
- `AgentConfig` (the local file) loses `intervals`; a file that still has `[intervals]` fails with the existing unknown-field error. `docs/install.md` and the example file drop the section.
- `build_scheduler` adds one `FilteredCollector` per group, each wrapping its own new collector instance, so rate-based collectors keep separate state per interval.
- Start-up: the watch channel starts with `ConfigCache::load()`. With no cache the agent collects nothing until the first successful fetch, and logs `waiting for the configuration from the server` once.
- `run_config_poller`: fetch at start and every `poll`. It reads the credential from `CredentialStore::load()` each time and skips the round while there is none (enrolment is the sender's job). A new configuration is saved to the cache and then sent on the channel. `Ok(None)`, `Retryable` and `Unauthorized` leave the current configuration in place; `Retryable` is logged at most once per failure streak.
- `run` supervises: whenever the channel changes to a configuration with a different `version`, cancel the running scheduler's child token, await it, build the new scheduler from `plan(items)` and start it. The spool writer, the sender and the channel to the writer live across restarts (the scheduler gets a clone of `tx`). A configuration whose `plan` is empty stops collection and logs a warning.
- Shutdown order stays as in M1: scheduler, then writer drains, then sender.

- [ ] **Step 1: Write the failing tests**

  - `config_rejects_intervals_section`.
  - `build_scheduler_one_entry_per_group`: two groups → the scheduler holds two collectors (expose a `len()` on `Scheduler` for tests).
  - Integration (`tests/reconfigure.rs`, wiremock server, temp state dir, real `run` with a short `poll` injected through a `run_with(config, poll, shutdown)` variant):
    - `collects_what_the_server_configures`: the server offers `system.uptime_seconds` at 1 s → within 5 s a `/values` request contains that key and no `cpu.usage`.
    - `reconfigures_without_restart`: the server then answers with version 2 containing only `memory.used_percent` at 1 s → later batches contain `memory.used_percent`; no `system.uptime_seconds` value has a timestamp more than 2 s after the switch.
    - `keeps_last_configuration_when_server_fails`: the config route starts returning 500 → values keep arriving.
    - `starts_from_cache_when_server_is_down`: a cache file with uptime at 1 s, no server → values land in the spool within 5 s (assert via `Spool::open` after shutdown).
    - `no_cache_and_no_server_collects_nothing`: after 3 s the spool is empty and the agent shuts down cleanly.
    - `unknown_item_from_newer_server_is_skipped`: items `gpu.usage` and `system.uptime_seconds` → uptime arrives.

- [ ] **Step 2: Run** `cargo test -p cerbrix-agent` — expected: the new tests fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p cerbrix-agent` — expected: all pass, including the M1 tests after their configuration fixtures drop `[intervals]`.
- [ ] **Step 5: Commit** `feat(agent): measure what the server configures and reconfigure live`

---

### Task 13: Packaging, end-to-end check and documentation

**Files:**
- Modify: `Dockerfile`, `docker-compose.yml`, `deploy/server.env.example`, `scripts/e2e.sh`, `docs/install.md`, `README.md`, `.github/workflows/ci.yml` (only if the e2e job needs `git` configuration)
- Create: `docs/configuration-repository.md`

**Interfaces:**
- Consumes: everything above.
- Produces: an image that can sync, an end-to-end check that goes through Git, and the operator documentation.

- [ ] **Step 1: Extend `scripts/e2e.sh` first** so it fails against the current image:
  1. Create a bare repository in the work directory from `examples/config-repo` plus `hosts/<e2e host>.json` (uses `linux-base`), branch `main`.
  2. Mount it read-only into the server container and set `CERBRIX_GITOPS__ENABLED=true`, `CERBRIX_GITOPS__URL=/config-repo.git`, `CERBRIX_GITOPS__INTERVAL_SECS=5`.
  3. Wait until `admin sync status` prints `in sync`; `admin host add` must fail with the "managed by Git" message.
  4. Create a key, start the agent without `[intervals]`, wait for `cerbrix_cpu_usage` and `cerbrix_disk_used_percent` as today.
  5. Commit a broken host file → within 30 s `admin sync status` prints `out of sync` and the file name; values keep arriving.
  6. Revert it → `in sync` again.
- [ ] **Step 2: Run** `scripts/e2e.sh` — expected: fails at step 3 (no `git` in the image).
- [ ] **Step 3: Update the packaging.** Runtime image installs `git` and `openssh-client`; a volume for `/var/lib/cerbrix/git`; the server process can write there. `deploy/server.env.example` gains the commented `CERBRIX_GITOPS__*` settings with the token line empty.
- [ ] **Step 4: Write the documentation.** `docs/configuration-repository.md`: directory layout, each file type with a complete example and a field table, template merge rules, item list link, the trigger expression syntax with the grammar and examples, validation rules, what Out Of Sync means and how to read it, the mass-removal guard, `check-config` in CI (a GitHub Actions snippet using the server image), schema use in editors (`$schema` and the VS Code `json.schemas` setting). `docs/install.md`: enabling GitOps with a token and with an SSH key, host key handling, the admin commands, that enabling GitOps deactivates hosts not in the repository. `README.md`: one paragraph and the link.
- [ ] **Step 5: Run** `scripts/e2e.sh`, then shellcheck and actionlint through their Docker images (`koalaman/shellcheck`, `rhysd/actionlint`) — expected: exit 0 each.
- [ ] **Step 6: Commit** `feat: GitOps in the Compose installation, end-to-end check through Git, documentation`
