# M1 – Metrics Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Linux agent measures CPU, memory, disk and network on a host, enrols with the server, and its values are stored in VictoriaMetrics and can be queried back; everything starts with `docker compose up`.

**Architecture:** A Cargo workspace with four crates. `cerbrix-agent` runs collectors on intervals, writes every value into an on-disk spool and ships batches over HTTPS. `cerbrix-server` authenticates agents against PostgreSQL, validates batches and writes them to a metrics store behind a trait whose first implementation is VictoriaMetrics. `cerbrix-common` holds the wire types both sides share. `cerbrix-proxy` exists only as an empty crate until v0.2.

**Tech Stack:** Rust (stable, edition 2024), tokio, axum, axum-server (rustls), sqlx (PostgreSQL, migrations), reqwest (rustls), serde, figment (TOML + environment), clap, sysinfo, tracing, thiserror/anyhow; tests with wiremock and tempfile. Add each dependency with `cargo add` at its latest stable version.

**Spec:** `docs/superpowers/specs/2026-11-14-cerbrix-design.md`

**Issues covered:** #1–#15 in `TrisMarzec/Cerbrix`. Each task names its issues; put `Closes #N` in the commit that completes one.

## Prerequisites

- Rust toolchain via rustup (not installed on the development machine at the time of writing) and a C linker.
- Docker (present) for PostgreSQL and VictoriaMetrics during tests: `docker compose -f docker-compose.dev.yml up -d`.
- Database tests read `DATABASE_URL` (`postgres://cerbrix:cerbrix@localhost:5432/cerbrix`).

## Global Constraints

- License Apache-2.0; every crate sets `license = "Apache-2.0"`.
- English for code, comments, documentation, commit messages.
- The agent (or proxy) always opens the connection; the server never connects to a host.
- No server state may exist only in process memory. Everything that must survive a restart or be shared by a second instance lives in PostgreSQL or the metrics store.
- Value ingest is stateless.
- Enrolment keys and agent credentials are stored only as SHA-256 hashes, never in plain text, never in a Git repository, never in logs.
- Every metric value carries its measurement time and is stored with it.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass after every task.
- Not in M1 (later milestones): hosts from Git, the agent fetching its item list from the server, triggers, UI, users. In M1 hosts and keys are created with the server's admin command line, and the agent's item intervals come from its own config file. The spec's item "agent reachable" is not measured by the agent; it is derived on the server from missing data in M3.
- The sizing target of 5,000 hosts per server instance is not load-tested in M1; M1 only keeps to the rules that make it reachable (stateless ingest, batched writes).

## Review Focus

1. **Metrics database down during ingest.** The server must answer 503 and the agent must keep the batch in its spool and retry; no value may be lost or duplicated in the spool. (Tests in Task 7 and Task 13.)
2. **Agent killed while writing the spool.** A truncated last line in a segment file must be skipped on restart; all complete lines before it are still delivered. (Test in Task 12.)
3. **Mount points and interface names with spaces, quotes, backslashes or non-ASCII characters.** They must round-trip unchanged from agent to a metrics query. (Tests in Task 2 and Task 6.)
4. **Agent clock far in the future or values that are NaN/infinite.** Those values are dropped and counted as rejected; the rest of the batch is accepted. (Tests in Task 2 and Task 7.)
5. **Second agent enrolling with a host name that is already bound, or enrolling with an expired or revoked key.** Rejected with a distinct error; the existing agent keeps working. (Tests in Task 5.)

---

## File Structure

```
Cargo.toml                         workspace, shared lints and dependency versions
LICENSE, README.md, CONTRIBUTING.md
.github/workflows/ci.yml           fmt, clippy, tests (PostgreSQL service)
.github/workflows/release.yml      agent binaries and server image on tags
docker-compose.yml                 server + PostgreSQL + VictoriaMetrics
docker-compose.dev.yml             PostgreSQL + VictoriaMetrics only, for development
Dockerfile                         server image
scripts/install-agent.sh           installs the agent as a systemd service
crates/cerbrix-common/src/
  lib.rs                           re-exports
  metric.rs                        MetricValue, ValueBatch, limits, validation
  enroll.rs                        EnrollRequest, EnrollResponse
  error.rs                         ErrorBody, ValidationError
crates/cerbrix-server/
  migrations/0001_m1.sql
  src/main.rs                      CLI: `serve`, `admin ...`
  src/config.rs                    Config and loading
  src/tokens.rs                    token generation and hashing
  src/state/mod.rs                 StateStore, connection, migrations
  src/state/hosts.rs               host queries
  src/state/enrollment.rs          keys, credentials, unknown agents
  src/enroll.rs                    enrolment rules
  src/metrics/mod.rs               MetricsStore trait, RangeQuery, Sample
  src/metrics/victoria.rs          VictoriaMetrics implementation
  src/api/mod.rs                   AppState, router, health
  src/api/agent.rs                 enrol and values handlers, bearer auth
  src/tls.rs                       listener setup
crates/cerbrix-agent/src/
  main.rs                          wiring, shutdown
  config.rs                        AgentConfig
  collector.rs                     Collector trait, Reading
  scheduler.rs                     interval scheduling
  collectors/system.rs             cpu, load, memory, swap, uptime
  collectors/disk.rs               per mount point
  collectors/net.rs                per interface
  collectors/filter.rs             exclusion patterns
  spool.rs                         on-disk buffer
  client.rs                        HTTP client for the server
  sender.rs                        spool → server loop with backoff
crates/cerbrix-proxy/src/main.rs   placeholder
```

---

### Task 1: Workspace, CI, license and project documents (#1, #2)

**Files:**
- Create: `Cargo.toml`, `crates/*/Cargo.toml`, `crates/cerbrix-common/src/lib.rs`, `crates/cerbrix-server/src/main.rs`, `crates/cerbrix-agent/src/main.rs`, `crates/cerbrix-proxy/src/main.rs`, `rust-toolchain.toml`, `.github/workflows/ci.yml`, `docker-compose.dev.yml`, `LICENSE`, `CONTRIBUTING.md`
- Modify: `README.md`, `.gitignore`

**Interfaces:**
- Produces: workspace members `cerbrix-common` (lib), `cerbrix-server`, `cerbrix-agent`, `cerbrix-proxy` (bins); `[workspace.package]` with `version = "0.1.0"`, `edition = "2024"`, `license = "Apache-2.0"`; `[workspace.lints.rust] unsafe_code = "forbid"`.

- [ ] **Step 1:** Create the workspace and the four crates. Each binary's `main` prints its name and version and exits 0. `cerbrix-proxy`'s `main` prints `cerbrix-proxy is planned for v0.2`.
- [ ] **Step 2:** `rust-toolchain.toml` with `channel = "stable"`, components `rustfmt`, `clippy`.
- [ ] **Step 3:** `docker-compose.dev.yml` with `postgres:17` (user, password and database all `cerbrix`, port 5432) and `victoriametrics/victoria-metrics` (port 8428), both with named volumes.
- [ ] **Step 4:** `ci.yml` on `push` and `pull_request`: job `check` on `ubuntu-latest` with a `postgres:17` service and `DATABASE_URL` set; steps `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Cache with `Swatinem/rust-cache`.
- [ ] **Step 5:** `LICENSE` with the unmodified Apache-2.0 text. `README.md`: what Cerbrix is (two sentences from spec section 1), status "early development, not usable yet", link to the design document and the project board. `CONTRIBUTING.md`: prerequisites, the three commands from Global Constraints, how to start the dev databases.
- [ ] **Step 6:** Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Expected: all pass, zero tests.
- [ ] **Step 7:** Commit `chore: set up workspace, CI and project documents` (Closes #1, Closes #2).

---

### Task 2: Shared wire types (#3)

**Files:**
- Create: `crates/cerbrix-common/src/{metric.rs,enroll.rs,error.rs}`
- Modify: `crates/cerbrix-common/src/lib.rs`

**Interfaces:**
- Produces:

```rust
pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_BATCH_VALUES: usize = 5000;
pub const MAX_PARAMS: usize = 8;
pub const MAX_PARAM_VALUE_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricValue {
    pub key: String,                          // e.g. "disk.used_percent"
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,     // e.g. {"mount": "/"}
    pub ts_ms: i64,                           // measurement time, Unix milliseconds
    pub value: f64,
}
impl MetricValue { pub fn validate(&self) -> Result<(), ValidationError>; }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueBatch { pub protocol_version: u32, pub values: Vec<MetricValue> }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestResponse { pub accepted: usize, pub rejected: usize }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollRequest { pub host: String, pub enrollment_key: String, pub agent_version: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollResponse { pub credential: String }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody { pub error: String, pub message: String }  // `error` is a stable machine code

pub fn validate_host_name(name: &str) -> Result<(), ValidationError>;
pub fn validate_item_key(key: &str) -> Result<(), ValidationError>;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ValidationError { HostName(String), ItemKey(String), ParamName(String), ParamValue(String), TooManyParams, NotFinite }
```

Rules: host name matches `^[A-Za-z0-9][A-Za-z0-9._-]{0,252}$`. Item key matches `^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$`. Param names match `^[a-z][a-z0-9_]*$`. Param values are 1 to `MAX_PARAM_VALUE_LEN` bytes of any UTF-8 without control characters. `value` must be finite.

- [ ] **Step 1: Write the failing tests** in `metric.rs` and `error.rs` test modules:

```rust
#[test] fn value_roundtrips_through_json_without_empty_params() {
    let v = MetricValue { key: "cpu.usage".into(), params: BTreeMap::new(), ts_ms: 1_700_000_000_000, value: 12.5 };
    let json = serde_json::to_string(&v).unwrap();
    assert_eq!(json, r#"{"key":"cpu.usage","ts_ms":1700000000000,"value":12.5}"#);
    assert_eq!(serde_json::from_str::<MetricValue>(&json).unwrap(), v);
}
#[test] fn odd_mount_points_roundtrip() {
    for mount in ["/mnt/my disk", r#"/mnt/"quoted""#, r"C:\Data", "/mnt/übung/日本"] {
        let v = value_with_param("disk.used_percent", "mount", mount);
        assert!(v.validate().is_ok());
        let back: MetricValue = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(back.params["mount"], mount);
    }
}
#[test] fn rejects_non_finite_values() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(value("cpu.usage", bad).validate(), Err(ValidationError::NotFinite));
    }
}
#[test] fn rejects_bad_keys_and_params() {
    assert!(validate_item_key("CPU.usage").is_err());
    assert!(validate_item_key("cpu..usage").is_err());
    assert!(validate_item_key("").is_err());
    assert!(value_with_param("disk.used_percent", "Mount", "/").validate().is_err());
    assert!(value_with_param("disk.used_percent", "mount", "a\nb").validate().is_err());
    assert!(value_with_param("disk.used_percent", "mount", &"x".repeat(257)).validate().is_err());
    assert_eq!(value_with_n_params(9).validate(), Err(ValidationError::TooManyParams));
}
#[test] fn host_names() {
    for ok in ["web-01", "db.internal.example", "A1"] { assert!(validate_host_name(ok).is_ok()); }
    for bad in ["", "-web", "web 01", "web/01", &"a".repeat(254)] { assert!(validate_host_name(bad).is_err()); }
}
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-common`. Expected: compile errors for the missing items.
- [ ] **Step 3:** Implement the types and validation functions above. Hand-written character checks, no regex dependency.
- [ ] **Step 4:** Run `cargo test -p cerbrix-common`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(common): shared wire types and validation` (Closes #3).

---

### Task 3: Server configuration (#13)

**Files:**
- Create: `crates/cerbrix-server/src/config.rs`
- Modify: `crates/cerbrix-server/src/main.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct Config { pub server: ServerConfig, pub database: DatabaseConfig, pub metrics: MetricsConfig, #[serde(default)] pub ingest: IngestConfig }
pub struct ServerConfig { pub listen: SocketAddr /* default 0.0.0.0:8443 */, pub tls_cert: Option<PathBuf>, pub tls_key: Option<PathBuf>, pub insecure_http: bool /* default false */ }
pub struct DatabaseConfig { pub url: String }
pub struct MetricsConfig { pub url: String }                      // VictoriaMetrics base URL
pub struct IngestConfig { pub max_future_skew_secs: u64 /* default 300 */, pub max_body_bytes: usize /* default 2 * 1024 * 1024 */ }

impl Config { pub fn load(path: Option<&Path>) -> Result<Config, ConfigError>; }
#[derive(Debug, thiserror::Error)] pub enum ConfigError { Read(String), Invalid(String) }
```

Sources, later wins: defaults, TOML file, environment variables with prefix `CERBRIX_` and `__` as the section separator (`CERBRIX_DATABASE__URL`). `load` fails with `Invalid` when: exactly one of `tls_cert`/`tls_key` is set; neither is set and `insecure_http` is false; `database.url` or `metrics.url` is empty or does not parse as a URL. The `Debug` output of `DatabaseConfig` must not contain the password.

- [ ] **Step 1: Write the failing tests** (use `figment::Jail` for file and environment isolation):

```rust
#[test] fn loads_file_and_env_overrides_file()        // file sets database.url=A, env CERBRIX_DATABASE__URL=B → B
#[test] fn defaults_apply()                            // listen == 0.0.0.0:8443, max_future_skew_secs == 300
#[test] fn rejects_missing_tls_without_insecure_flag() // Err(Invalid(msg)) and msg names `server.insecure_http`
#[test] fn rejects_cert_without_key()                  // Err(Invalid(msg)) and msg names `server.tls_key`
#[test] fn rejects_unparseable_urls()                  // metrics.url = "not a url" → Err(Invalid(msg)) naming `metrics.url`
#[test] fn debug_hides_database_password()             // format!("{:?}", cfg) does not contain "s3cret"
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-server config`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement `Config::load` with figment.
- [ ] **Step 4:** In `main.rs` add the clap CLI skeleton: global `--config <PATH>` (env `CERBRIX_CONFIG`), subcommands `serve` and `admin` (bodies come in later tasks). On `ConfigError` print `configuration error: <message>` to stderr and exit with code 2.
- [ ] **Step 5:** Run `cargo test -p cerbrix-server config`. Expected: PASS.
- [ ] **Step 6:** Commit `feat(server): configuration from file and environment` (Closes #13).

---

### Task 4: PostgreSQL state store (#10)

**Files:**
- Create: `crates/cerbrix-server/migrations/0001_m1.sql`, `crates/cerbrix-server/src/tokens.rs`, `crates/cerbrix-server/src/state/{mod.rs,hosts.rs,enrollment.rs}`

**Interfaces:**
- Produces:

```rust
// tokens.rs
pub const ENROLLMENT_KEY_PREFIX: &str = "cbx_ek_";
pub const AGENT_CREDENTIAL_PREFIX: &str = "cbx_ac_";
pub fn generate(prefix: &str) -> String;   // prefix + 32 random bytes, base64url without padding
pub fn hash(token: &str) -> [u8; 32];      // SHA-256

// state
#[derive(Clone)] pub struct StateStore { /* PgPool */ }
#[derive(Debug, Clone, PartialEq)] pub struct Host { pub id: Uuid, pub name: String, pub active: bool }
pub struct UnknownAgent { pub host_name: String, pub first_seen: OffsetDateTime, pub last_seen: OffsetDateTime, pub attempts: i64 }
#[derive(Debug, thiserror::Error)] pub enum StateError { Conflict(String), NotFound, Db(#[from] sqlx::Error) }

impl StateStore {
    pub async fn connect(url: &str) -> Result<Self, StateError>;          // runs embedded migrations
    pub fn from_pool(pool: PgPool) -> Self;                               // for tests
    pub async fn ping(&self) -> Result<(), StateError>;
    pub async fn create_host(&self, name: &str) -> Result<Host, StateError>;           // Conflict when the name exists
    pub async fn host_by_name(&self, name: &str) -> Result<Option<Host>, StateError>;
    pub async fn set_host_active(&self, id: Uuid, active: bool) -> Result<(), StateError>;
    pub async fn create_enrollment_key(&self, description: &str, expires_at: OffsetDateTime) -> Result<(Uuid, String), StateError>; // returns id and the plain key, shown once
    pub async fn revoke_enrollment_key(&self, id: Uuid) -> Result<(), StateError>;
    pub async fn enrollment_key_valid(&self, plain: &str, now: OffsetDateTime) -> Result<bool, StateError>;
    pub async fn bind_credential(&self, host_id: Uuid) -> Result<String, StateError>;  // Conflict when already bound; returns plain credential
    pub async fn release_binding(&self, host_id: Uuid) -> Result<(), StateError>;
    pub async fn host_for_credential(&self, plain: &str) -> Result<Option<Host>, StateError>; // only active hosts
    pub async fn record_unknown_agent(&self, host_name: &str, now: OffsetDateTime) -> Result<(), StateError>;
    pub async fn unknown_agents(&self) -> Result<Vec<UnknownAgent>, StateError>;
}
```

Tables: `hosts(id uuid pk, name text unique not null, active boolean not null default true, created_at timestamptz)`, `enrollment_keys(id uuid pk, key_hash bytea unique not null, description text not null, expires_at timestamptz not null, revoked_at timestamptz, created_at timestamptz)`, `agent_credentials(host_id uuid pk references hosts on delete cascade, credential_hash bytea unique not null, enrolled_at timestamptz)`, `unknown_agents(host_name text pk, first_seen timestamptz, last_seen timestamptz, attempts bigint)`. Host names compare case-sensitively. `bind_credential` relies on the primary key for the conflict, not on a read followed by a write, so two instances cannot both bind.

- [ ] **Step 1: Write the failing tests** with `#[sqlx::test(migrations = "./migrations")]`:

```rust
async fn create_host_twice_conflicts(pool)             // second call → Err(StateError::Conflict(_))
async fn key_is_stored_hashed(pool)                    // no row in enrollment_keys contains the plain key bytes; key starts with "cbx_ek_"
async fn key_validity(pool)                            // valid before expiry; false at expires_at; false after revoke; false for an unknown key
async fn bind_twice_conflicts_and_release_allows_rebind(pool)
async fn concurrent_bind_yields_exactly_one_success(pool)   // tokio::join! two binds for one host → one Ok, one Conflict
async fn credential_resolves_only_active_hosts(pool)   // Some(host); after set_host_active(false) → None; garbage token → None
async fn unknown_agent_attempts_accumulate(pool)       // two records → attempts == 2, first_seen < last_seen
#[test] fn tokens_are_unique_and_hash_is_stable()      // generate twice differs; hash("a") == hash("a") != hash("b")
```

- [ ] **Step 2:** Start the dev database and run `cargo test -p cerbrix-server state`. Expected: FAIL to compile.
- [ ] **Step 3:** Write the migration and implement `tokens.rs` and `StateStore`.
- [ ] **Step 4:** Run `cargo test -p cerbrix-server state tokens`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(server): PostgreSQL state store with migrations` (Closes #10).

---

### Task 5: Enrolment rules and admin commands (#11, server side)

**Files:**
- Create: `crates/cerbrix-server/src/enroll.rs`
- Modify: `crates/cerbrix-server/src/main.rs`

**Interfaces:**
- Consumes: `StateStore` (Task 4), `EnrollRequest`, `EnrollResponse`, `validate_host_name` (Task 2).
- Produces:

```rust
pub async fn enroll(store: &StateStore, req: &EnrollRequest, now: OffsetDateTime) -> Result<EnrollResponse, EnrollError>;
#[derive(Debug, thiserror::Error)]
pub enum EnrollError { InvalidHostName, InvalidKey, UnknownHost, AlreadyBound, State(#[from] StateError) }
impl EnrollError { pub fn code(&self) -> &'static str; }  // "invalid_host_name" | "invalid_key" | "unknown_host" | "already_bound" | "internal"
```

Order of checks: host name format, then key validity, then host lookup. Only a request with a valid key and a missing or inactive host is recorded as an unknown agent, so the list cannot be filled by callers without a key.

Admin commands (print results to stdout, errors to stderr with exit code 1):
- `cerbrix-server admin host add <NAME>`
- `cerbrix-server admin host release <NAME>` – removes the agent binding
- `cerbrix-server admin key create --description <TEXT> --expires-in <DURATION>` – duration like `30d`, `12h`; prints the key once
- `cerbrix-server admin key revoke <ID>`
- `cerbrix-server admin unknown-agents`

- [ ] **Step 1: Write the failing tests** (`#[sqlx::test]`):

```rust
async fn enrols_known_host_with_valid_key(pool)        // Ok; credential starts with "cbx_ac_"; host_for_credential resolves to the host
async fn expired_key_is_invalid(pool)                  // Err(InvalidKey)
async fn revoked_key_is_invalid(pool)                  // Err(InvalidKey)
async fn second_enrolment_is_rejected_and_first_credential_still_works(pool)   // Err(AlreadyBound); first credential still resolves
async fn release_allows_new_enrolment_and_invalidates_old_credential(pool)
async fn unknown_host_is_recorded_only_with_valid_key(pool)   // valid key → Err(UnknownHost), unknown_agents has 1 row; invalid key → Err(InvalidKey), still 1 row
async fn inactive_host_counts_as_unknown(pool)
async fn bad_host_name_is_rejected_before_key_check(pool)     // "web 01" → Err(InvalidHostName)
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-server enroll`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement `enroll` and the admin subcommands.
- [ ] **Step 4:** Run `cargo test -p cerbrix-server enroll`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(server): agent enrolment rules and admin commands`.

---

### Task 6: Metrics store with VictoriaMetrics backend (#9)

**Files:**
- Create: `crates/cerbrix-server/src/metrics/{mod.rs,victoria.rs}`

**Interfaces:**
- Consumes: `MetricValue` (Task 2).
- Produces:

```rust
#[async_trait::async_trait]
pub trait MetricsStore: Send + Sync {
    async fn write(&self, host: &str, values: &[MetricValue]) -> Result<(), MetricsError>;
    async fn query_range(&self, query: &RangeQuery) -> Result<Vec<Sample>, MetricsError>;
}
pub struct RangeQuery { pub host: String, pub key: String, pub params: BTreeMap<String, String>, pub from_ms: i64, pub to_ms: i64, pub step_secs: u32 }
#[derive(Debug, Clone, PartialEq)] pub struct Sample { pub ts_ms: i64, pub value: f64 }
#[derive(Debug, thiserror::Error)] pub enum MetricsError { Unavailable(String), Rejected(String) }

pub struct VictoriaMetrics { /* reqwest::Client, base Url */ }
impl VictoriaMetrics { pub fn new(base_url: &str) -> Result<Self, MetricsError>; }

pub struct MemoryMetrics;   // #[cfg(any(test, feature = "test-util"))], stores writes in a Mutex<Vec<(String, MetricValue)>>, can be switched to fail
```

Mapping: series name is `cerbrix_` + key with `.` replaced by `_` (`disk.used_percent` → `cerbrix_disk_used_percent`). Labels: `host` = host name, and each param as `p_<name>` (`p_mount`). Write with `POST {base}/api/v1/import`, one JSON line per value (`{"metric":{...},"values":[v],"timestamps":[ts_ms]}`). Read with `GET {base}/api/v1/query_range` using a selector built from the same mapping; label values in the selector are escaped for `\`, `"` and newline. `Unavailable` for connection errors, timeouts and 5xx; `Rejected` for 4xx. Request timeout 10 seconds.

- [ ] **Step 1: Write the failing tests** against a `wiremock::MockServer`:

```rust
async fn write_sends_one_import_line_per_value()        // body has 2 lines; first parses to metric {"__name__":"cerbrix_cpu_usage","host":"web-01"}, values [12.5], timestamps [1700000000000]
async fn params_become_prefixed_labels()                // {"mount":"/"} → label "p_mount":"/"
async fn odd_label_values_survive_write_and_query()     // mount `/mnt/"my" disk\x` appears unchanged in the import line and correctly escaped in the query selector
async fn query_range_parses_matrix_result()             // mocked response with 2 points → vec![Sample{..}, Sample{..}] in time order
async fn server_error_is_unavailable()                  // 500 → Err(Unavailable(_))
async fn bad_request_is_rejected()                      // 400 → Err(Rejected(_))
async fn empty_write_makes_no_request()
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-server metrics`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement the trait, `VictoriaMetrics` and `MemoryMetrics`.
- [ ] **Step 4:** Run `cargo test -p cerbrix-server metrics`. Expected: PASS.
- [ ] **Step 5:** Add one ignored integration test `roundtrip_against_real_victoriametrics` that writes two values and reads them back from `http://localhost:8428` (run with `cargo test -p cerbrix-server -- --ignored` after starting the dev compose file). Expected: PASS.
- [ ] **Step 6:** Commit `feat(server): metrics store interface with VictoriaMetrics backend` (Closes #9).

---

### Task 7: Agent API – enrol, ingest, health (#8, #11)

**Files:**
- Create: `crates/cerbrix-server/src/api/{mod.rs,agent.rs}`

**Interfaces:**
- Consumes: `StateStore`, `enroll` (Task 5), `MetricsStore` (Task 6), `Config` (Task 3), wire types (Task 2).
- Produces:

```rust
#[derive(Clone)] pub struct AppState { pub store: StateStore, pub metrics: Arc<dyn MetricsStore>, pub config: Arc<Config> }
pub fn router(state: AppState) -> axum::Router;
```

Routes:

| Route | Behaviour |
|---|---|
| `GET /healthz` | 200 `{"status":"ok"}` when `store.ping()` succeeds, else 503 |
| `POST /api/agent/v1/enroll` | body `EnrollRequest`. 200 `EnrollResponse`; `invalid_host_name` → 400, `invalid_key` → 401, `unknown_host` → 404, `already_bound` → 409 |
| `POST /api/agent/v1/values` | header `Authorization: Bearer <credential>`, body `ValueBatch`. 202 `IngestResponse` |

Ingest rules, in this order: missing or unknown credential → 401 `unauthorized`; body larger than `ingest.max_body_bytes` → 413 `body_too_large`; unparseable JSON → 400 `malformed`; `protocol_version != PROTOCOL_VERSION` → 400 `unsupported_protocol`; more than `MAX_BATCH_VALUES` values → 413 `batch_too_large`. Then each value is checked on its own: failing `validate()` or `ts_ms` more than `max_future_skew_secs` ahead of the server clock drops that value and counts it as rejected. Accepted values go to `metrics.write(host.name, ..)`; `MetricsError::Unavailable` → 503 `metrics_unavailable`, `Rejected` → 500 `internal`. All error bodies are `ErrorBody`. The handler keeps nothing in memory between requests. Credentials never appear in logs or error messages.

- [ ] **Step 1: Write the failing tests** (`#[sqlx::test]`, `MemoryMetrics`, requests through `tower::ServiceExt::oneshot`):

```rust
async fn healthz_ok(pool)
async fn enroll_then_send_values_stores_them_under_the_host(pool)   // 202 {accepted:2,rejected:0}; MemoryMetrics holds 2 values for "web-01"
async fn values_without_credential_are_unauthorized(pool)           // 401, nothing written
async fn values_with_credential_of_deactivated_host_are_unauthorized(pool)
async fn wrong_protocol_version_is_rejected(pool)                   // 400 "unsupported_protocol"
async fn oversized_batch_is_rejected(pool)                          // 5001 values → 413 "batch_too_large", nothing written
async fn future_and_invalid_values_are_dropped_not_fatal(pool)      // batch of: 1 good, 1 with ts now+1h, 1 with key "BAD" → 202 {accepted:1,rejected:2}
async fn metrics_outage_returns_503_and_writes_nothing(pool)        // MemoryMetrics failing → 503 "metrics_unavailable"
async fn enroll_error_codes_map_to_statuses(pool)                   // 400 / 401 / 404 / 409 with the codes above
async fn second_router_instance_serves_the_same_credential(pool)    // two `router()` built on one pool both accept the credential
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-server api`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement the router and handlers. Bearer authentication as an axum extractor that yields `Host`.
- [ ] **Step 4:** Run `cargo test -p cerbrix-server api`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(server): agent API for enrolment and value ingest` (Closes #8).

---

### Task 8: Server start-up and TLS (#12, server side)

**Files:**
- Create: `crates/cerbrix-server/src/tls.rs`
- Modify: `crates/cerbrix-server/src/main.rs`

**Interfaces:**
- Consumes: `Config`, `router`, `StateStore::connect`, `VictoriaMetrics::new`.
- Produces, in `tls.rs`: `pub async fn serve(config: Config, listener: std::net::TcpListener, shutdown: impl Future<Output = ()> + Send + 'static) -> anyhow::Result<()>`. `main.rs` binds `config.server.listen` and passes the listener; tests bind `127.0.0.1:0` and read the port from the listener before passing it.

Behaviour: with `tls_cert` and `tls_key` the server speaks HTTPS only (rustls via `axum-server`). With `insecure_http = true` and no certificate it speaks plain HTTP and logs one warning at start: `serving plain HTTP; use only for development or behind a TLS-terminating reverse proxy`. Unreadable or mismatched certificate files stop the start with an error naming the file. `serve` shuts down gracefully on Ctrl-C and SIGTERM.

- [ ] **Step 1: Write the failing tests** (`tests/tls.rs`; generate a self-signed certificate with `rcgen` into a temp directory, bind to port 0):

```rust
async fn https_healthz_works_with_supplied_certificate()   // reqwest with that cert as root → 200
async fn plain_http_is_refused_when_tls_is_configured()    // http:// request to the same port fails or is not 200
async fn missing_certificate_file_fails_at_start()         // error message contains the path
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-server --test tls`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement `tls.rs` and `serve`; wire the `serve` subcommand in `main.rs`.
- [ ] **Step 4:** Run `cargo test -p cerbrix-server`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(server): start-up with HTTPS from operator-supplied certificate`.

---

### Task 9: Agent collector framework and scheduler (#4)

**Files:**
- Create: `crates/cerbrix-agent/src/{collector.rs,scheduler.rs}`

**Interfaces:**
- Consumes: `MetricValue`.
- Produces:

```rust
pub struct Reading { pub key: &'static str, pub params: BTreeMap<String, String>, pub value: f64 }
pub trait Collector: Send {
    fn name(&self) -> &'static str;
    fn collect(&mut self) -> anyhow::Result<Vec<Reading>>;
}
pub struct Scheduler { /* .. */ }
impl Scheduler {
    pub fn new(out: tokio::sync::mpsc::Sender<MetricValue>) -> Self;
    pub fn add(&mut self, collector: Box<dyn Collector>, interval: Duration);
    pub async fn run(self, shutdown: tokio_util::sync::CancellationToken);
}
```

Each collector runs on its own interval, first run immediately. `collect` runs on the blocking pool. A collector that returns an error or panics is logged with its name and tried again at its next tick; the others are unaffected. Missed ticks are skipped, not caught up. `ts_ms` is taken when `collect` returns. Non-finite readings are dropped with a warning.

- [ ] **Step 1: Write the failing tests** (`#[tokio::test(start_paused = true)]`, fake collectors):

```rust
async fn collectors_run_on_their_own_intervals()     // A every 10s, B every 30s; advance 60s → A produced 7 values, B 3
async fn failing_collector_does_not_stop_others()    // A always Err, B fine → B's values still arrive
async fn panicking_collector_does_not_stop_others()
async fn non_finite_readings_are_dropped()
async fn run_ends_on_shutdown()
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-agent scheduler`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement `Collector`, `Reading`, `Scheduler`.
- [ ] **Step 4:** Run `cargo test -p cerbrix-agent scheduler`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(agent): collector framework and scheduler` (Closes #4).

---

### Task 10: System collectors (#5)

**Files:**
- Create: `crates/cerbrix-agent/src/collectors/{mod.rs,system.rs}`, `docs/items.md`

**Interfaces:**
- Consumes: `Collector`, `Reading`.
- Produces: `CpuCollector`, `LoadCollector`, `MemoryCollector`, `UptimeCollector`, each with `pub fn new() -> Self`.

Item keys (no params):

| Collector | Keys |
|---|---|
| `CpuCollector` | `cpu.usage` (percent 0–100, all cores averaged, measured between two consecutive `collect` calls; the first call returns no reading) |
| `LoadCollector` | `system.load1`, `system.load5`, `system.load15` |
| `MemoryCollector` | `memory.total_bytes`, `memory.used_bytes`, `memory.available_bytes`, `memory.used_percent`, `swap.total_bytes`, `swap.used_bytes`, `swap.used_percent` (omitted when total swap is 0) |
| `UptimeCollector` | `system.uptime_seconds` |

Use the `sysinfo` crate. `docs/items.md` lists every key with unit and meaning.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test] fn cpu_first_call_is_empty_then_reports_percent()   // 1st collect → []; sleep 250ms; 2nd → one reading, 0.0..=100.0
#[test] fn memory_readings_are_consistent()                  // used + available <= total * 1.05; used_percent within 0..=100
#[test] fn uptime_is_positive()
#[test] fn all_keys_are_valid_item_keys()                    // every key passes cerbrix_common::validate_item_key
#[test] #[cfg(target_os = "linux")] fn load_reports_three_values()
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-agent system`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement the four collectors and write `docs/items.md`.
- [ ] **Step 4:** Run `cargo test -p cerbrix-agent system`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(agent): CPU, load, memory, swap and uptime items` (Closes #5).

---

### Task 11: Disk and network collectors with instance discovery (#6)

**Files:**
- Create: `crates/cerbrix-agent/src/collectors/{disk.rs,net.rs,filter.rs}`
- Modify: `docs/items.md`

**Interfaces:**
- Consumes: `Collector`, `Reading`.
- Produces:

```rust
// filter.rs
pub struct Patterns(/* .. */);
impl Patterns { pub fn new(globs: &[String]) -> anyhow::Result<Self>; pub fn matches(&self, s: &str) -> bool; }  // `*` and `?` wildcards

// disk.rs
pub const DEFAULT_EXCLUDED_FS: &[&str] = &["proc","sysfs","devtmpfs","devpts","tmpfs","cgroup","cgroup2","overlay","squashfs","autofs","debugfs","tracefs","securityfs","pstore","bpf","mqueue","hugetlbfs","fusectl","configfs","ramfs","nsfs"];
pub struct DiskCollector { /* .. */ }
impl DiskCollector { pub fn new(exclude_fs: Vec<String>, exclude_mounts: Patterns) -> Self; }

// net.rs
pub struct NetCollector { /* .. */ }
impl NetCollector { pub fn new(exclude_interfaces: Patterns) -> Self; }   // loopback is always excluded
```

Keys: `disk.total_bytes`, `disk.used_bytes`, `disk.used_percent` with param `mount`; `net.rx_bytes_per_sec`, `net.tx_bytes_per_sec` with param `interface` (rate between two consecutive `collect` calls; the first call returns no reading; a counter that went backwards yields no reading for that interface). Discovery happens on every `collect`, so mounts and interfaces that appear or disappear are picked up without a restart. A mount point listed twice is reported once. Disks with total size 0 are skipped.

Both collectors take their raw data through a small internal trait (`DiskSource`, `NetSource`) so tests can feed fixed lists; the production implementation reads from `sysinfo`.

- [ ] **Step 1: Write the failing tests** with fake sources:

```rust
#[test] fn reports_three_keys_per_mount()                    // mounts "/" (100 total, 40 used) and "/data" → 6 readings; "/" used_percent == 40.0
#[test] fn excludes_default_pseudo_filesystems()             // a "tmpfs" at "/run" yields nothing
#[test] fn excludes_mounts_by_pattern()                      // pattern "/var/lib/docker/*" hides "/var/lib/docker/overlay2/x"
#[test] fn duplicate_mount_points_are_reported_once()
#[test] fn zero_sized_disks_are_skipped()
#[test] fn new_mount_appears_without_restart()               // source changes between two collects
#[test] fn net_first_call_is_empty_then_reports_rates()      // counters +1000 bytes over 2s → 500.0
#[test] fn net_counter_reset_yields_no_reading()
#[test] fn loopback_is_always_excluded_and_patterns_apply()  // "lo" hidden; pattern "veth*" hides "veth12ab"
#[test] fn patterns_match_wildcards()                        // "veth*" matches "veth1", not "eth0"; "eth?" matches "eth0", not "eth10"
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-agent disk net filter`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement `Patterns`, `DiskCollector`, `NetCollector` and extend `docs/items.md`.
- [ ] **Step 4:** Run `cargo test -p cerbrix-agent`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(agent): disk and network items with instance discovery` (Closes #6).

---

### Task 12: On-disk spool (#7)

**Files:**
- Create: `crates/cerbrix-agent/src/spool.rs`

**Interfaces:**
- Consumes: `MetricValue`.
- Produces:

```rust
pub struct Spool { /* .. */ }
impl Spool {
    pub fn open(dir: &Path, max_bytes: u64) -> io::Result<Self>;
    pub fn push(&mut self, values: &[MetricValue]) -> io::Result<()>;
    pub fn peek(&mut self, max: usize) -> io::Result<Vec<MetricValue>>;   // oldest first, does not remove
    pub fn ack(&mut self, count: usize) -> io::Result<()>;                // removes the `count` oldest values returned by the last peek
    pub fn size_bytes(&self) -> u64;
    pub fn is_empty(&self) -> bool;
}
pub const SEGMENT_BYTES: u64 = 1024 * 1024;
```

Layout: numbered segment files `00000001.jsonl`, one `MetricValue` as JSON per line, a new segment once the current one reaches `SEGMENT_BYTES`; a `cursor` file holding segment number and line offset of the oldest unacknowledged value, replaced atomically (write to a temp file, then rename). When the total size exceeds `max_bytes`, whole oldest segments are deleted (never the one being written) and a warning with the number of dropped values is logged. On `open`, a line that is not valid JSON is skipped with a warning; fully acknowledged segments are deleted.

- [ ] **Step 1: Write the failing tests** (`tempfile::tempdir`):

```rust
#[test] fn push_peek_ack_in_order()                         // push 5, peek(3) → first 3, ack(3), peek(10) → last 2
#[test] fn peek_without_ack_returns_the_same_values()
#[test] fn survives_reopen()                                // push 5, ack 2, drop, open again → peek returns the remaining 3
#[test] fn truncated_last_line_is_skipped_on_reopen()       // append half a JSON line to the newest segment → the complete values are still returned, no error
#[test] fn rotates_segments_and_drops_oldest_over_limit()   // max_bytes = 3 * SEGMENT_BYTES, push ~5 segments worth → size_bytes <= max_bytes and peek starts at a later value
#[test] fn acknowledged_segments_are_deleted()
#[test] fn ack_more_than_peeked_is_an_error()
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-agent spool`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement `Spool`.
- [ ] **Step 4:** Run `cargo test -p cerbrix-agent spool`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(agent): on-disk spool for values` (Closes #7).

---

### Task 13: Agent configuration, server client and main loop (#11, #12 agent side)

**Files:**
- Create: `crates/cerbrix-agent/src/{config.rs,client.rs,sender.rs}`
- Modify: `crates/cerbrix-agent/src/main.rs`

**Interfaces:**
- Consumes: `Scheduler`, collectors, `Spool`, wire types, `Patterns`.
- Produces:

```rust
// config.rs
pub struct AgentConfig {
    pub server_url: Url,                      // must be https unless allow_insecure_http
    pub host: String,
    pub enrollment_key: Option<String>,
    pub ca_file: Option<PathBuf>,
    pub allow_insecure_http: bool,            // default false
    pub state_dir: PathBuf,                   // default /var/lib/cerbrix-agent
    pub spool_max_bytes: u64,                 // default 64 MiB
    pub intervals: Intervals,                 // cpu 30s, load 30s, memory 30s, uptime 5m, disk 5m, net 30s
    pub disk: DiskOptions,                    // exclude_fs (default DEFAULT_EXCLUDED_FS), exclude_mounts
    pub net: NetOptions,                      // exclude_interfaces
}
#[derive(Debug, thiserror::Error)] pub enum ConfigError { Read(String), Invalid(String) }   // the agent's own type, same shape as the server's
impl AgentConfig { pub fn load(path: Option<&Path>) -> Result<Self, ConfigError>; }   // TOML file (default /etc/cerbrix/agent.toml) + env prefix CERBRIX_AGENT_, `__` separator

// client.rs
pub struct ServerClient { /* .. */ }
impl ServerClient {
    pub fn new(cfg: &AgentConfig) -> anyhow::Result<Self>;                              // adds ca_file as an extra root; timeout 15s
    pub async fn enroll(&self, host: &str, key: &str) -> Result<String, ClientError>;
    pub async fn send(&self, credential: &str, values: &[MetricValue]) -> Result<IngestResponse, ClientError>;
}
#[derive(Debug, thiserror::Error)] pub enum ClientError { Retryable(String), Unauthorized, Refused { code: String, message: String } }

// sender.rs
pub async fn run_sender(client: ServerClient, spool: Arc<Mutex<Spool>>, credential: CredentialStore, shutdown: CancellationToken);
pub struct CredentialStore { /* state_dir/credential */ }
impl CredentialStore { pub fn load(&self) -> io::Result<Option<String>>; pub fn save(&self, credential: &str) -> io::Result<()>; pub fn clear(&self) -> io::Result<()>; }
```

Behaviour:
- `ClientError` mapping: connection errors, timeouts, 5xx and 429 → `Retryable`; 401 → `Unauthorized`; other 4xx → `Refused` with the `ErrorBody` fields.
- Start-up: load the stored credential; if there is none, enrol with `enrollment_key` and save the credential (file mode 0600 on Unix). Without credential and without key the agent exits with code 2 and a message naming `enrollment_key`. `Refused` on enrolment (`unknown_host`, `already_bound`, `invalid_key`) is logged with its code and retried every 60 seconds, because the host entry may appear later.
- Values flow scheduler → channel → spool. The sender peeks up to 1000 values, sends, and acknowledges only after a 202. `Retryable` → keep the values, exponential backoff from 1 s doubling to a cap of 60 s, reset after a success. `Unauthorized` → clear the credential and go back to enrolment; values stay in the spool. `Refused` on send → log, acknowledge the batch (it can never succeed) and continue.
- Shutdown on SIGTERM/Ctrl-C: stop the scheduler, flush the channel into the spool, exit 0.
- The credential and the enrolment key never appear in logs.

- [ ] **Step 1: Write the failing tests** (`wiremock`, `tempfile`, paused time where backoff matters):

```rust
#[test] fn config_rejects_http_url_without_insecure_flag()
#[test] fn config_defaults_apply()
async fn enroll_returns_credential()
async fn client_maps_statuses()                              // 503 → Retryable, 401 → Unauthorized, 409 already_bound → Refused{code:"already_bound",..}
async fn client_trusts_custom_ca_file()                      // HTTPS mock with rcgen cert: fails without ca_file, succeeds with it
async fn sender_acks_only_after_202()                        // server returns 503 twice, then 202 → exactly one batch stored, spool empty, values identical to what was pushed
async fn sender_keeps_values_while_server_is_down()          // server always 503 for 5 minutes → spool still holds all values, attempts follow 1,2,4,…,60s spacing
async fn sender_reenrols_after_unauthorized()                // 401 on send → credential cleared → enrol called again → values delivered
async fn refused_batch_is_dropped_and_sending_continues()
#[cfg(unix)] #[test] fn credential_file_is_private()         // mode & 0o077 == 0
```

- [ ] **Step 2:** Run `cargo test -p cerbrix-agent config client sender`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement `AgentConfig`, `ServerClient`, `CredentialStore`, `run_sender`, and wire everything in `main.rs` (`cerbrix-agent --config <PATH>`).
- [ ] **Step 4:** Run `cargo test -p cerbrix-agent`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(agent): configuration, server client and delivery loop` (Closes #11, Closes #12).

---

### Task 14: Docker Compose installation and end-to-end check (#14)

**Files:**
- Create: `Dockerfile`, `docker-compose.yml`, `.dockerignore`, `deploy/server.env.example`, `scripts/e2e.sh`, `docs/install.md`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: the `cerbrix-server` binary and its `CERBRIX_*` environment variables (Task 3), admin commands (Task 5).

Details:
- `Dockerfile`: multi-stage, build with the official `rust` image, run on `debian:stable-slim` as a non-root user, `ENTRYPOINT ["cerbrix-server"]`, `CMD ["serve"]`, `HEALTHCHECK` against `/healthz`.
- `docker-compose.yml`: services `postgres` (`postgres:17`, health check), `victoriametrics` (retention flag `-retentionPeriod=12` months), `server` (built from the Dockerfile, depends on both being healthy, port 8443). Configuration through `deploy/server.env` (copied from the example). The example ships with `CERBRIX_SERVER__INSECURE_HTTP=true` and a comment explaining how to mount a certificate and set `CERBRIX_SERVER__TLS_CERT`/`TLS_KEY` instead. No default database password in the compose file; it comes from the env file.
- `scripts/e2e.sh`: starts the stack, runs `admin host add e2e-host` and `admin key create` inside the server container, starts the locally built agent with a temp state directory and 2-second intervals, waits up to 60 seconds, then queries VictoriaMetrics for `cerbrix_cpu_usage{host="e2e-host"}` and `cerbrix_disk_used_percent{host="e2e-host"}` and fails unless both return at least one sample. Always tears the stack down.
- `docs/install.md`: the steps a new operator follows: copy the env file, `docker compose up -d`, create a host and a key, install an agent.

- [ ] **Step 1:** Write `scripts/e2e.sh` first. Run it. Expected: FAIL (no compose file).
- [ ] **Step 2:** Write `Dockerfile`, `.dockerignore`, `docker-compose.yml`, `deploy/server.env.example`.
- [ ] **Step 3:** Run `scripts/e2e.sh`. Expected: exits 0 and prints the sample counts for both series.
- [ ] **Step 4:** Add a CI job `e2e` on `ubuntu-latest` that runs `scripts/e2e.sh`.
- [ ] **Step 5:** Write `docs/install.md` and link it from `README.md`.
- [ ] **Step 6:** Commit `feat: Docker Compose installation and end-to-end check` (Closes #14).

---

### Task 15: Agent release binaries and install script (#15)

**Files:**
- Create: `.github/workflows/release.yml`, `scripts/install-agent.sh`, `deploy/cerbrix-agent.service`
- Modify: `docs/install.md`

Details:
- `release.yml` on tags `v*`: build `cerbrix-agent` for `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` (with `cross`), package each as `cerbrix-agent-<version>-<target>.tar.gz` with a `.sha256` file, attach them to a GitHub release; build and push the server image to `ghcr.io/trismarzec/cerbrix-server:<version>`.
- `deploy/cerbrix-agent.service`: runs as a dedicated `cerbrix` system user, `Restart=on-failure`, `StateDirectory=cerbrix-agent`, `ProtectSystem=strict`, `NoNewPrivileges=true`.
- `scripts/install-agent.sh` (POSIX sh): options `--server-url`, `--host`, `--enrollment-key`, `--ca-file`, `--version` (default latest). It detects the architecture, downloads the archive and checksum, verifies the checksum and aborts on mismatch, installs the binary to `/usr/local/bin`, creates the `cerbrix` user, writes `/etc/cerbrix/agent.toml` with mode 0640 owned by `root:cerbrix`, installs and starts the unit. It refuses to run without root, on systems without systemd, and on unsupported architectures, each with a one-line message. It never prints the enrolment key. Running it again upgrades the binary and keeps an existing config file. `--dry-run` prints the actions without performing them and skips the root check.

- [ ] **Step 1:** Write `scripts/install-agent.sh` and the unit file.
- [ ] **Step 2:** Run `shellcheck scripts/install-agent.sh scripts/e2e.sh`. Expected: no findings. Add this as a CI step.
- [ ] **Step 3:** Run `sh scripts/install-agent.sh --dry-run --server-url https://example.test --host web-01 --enrollment-key x`. Expected: the planned steps are listed and the key value is not in the output.
- [ ] **Step 4:** Write `release.yml`. Validate it with `actionlint`. Expected: no findings.
- [ ] **Step 5:** Extend `docs/install.md` with the agent installation command.
- [ ] **Step 6:** Commit `feat: agent release binaries and install script` (Closes #15).

---

## After the last task

- Run the full check once more: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && scripts/e2e.sh`.
- Move the M1 issues on the project board to "Done" as their closing commits land.
- The release workflow itself can only be verified by pushing a tag; do that only when the project owner asks for a release.
