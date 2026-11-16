# M4a – Server API and Login Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The server offers a documented HTTP API with login, three roles and read-only display tokens, through which a web UI can show hosts, graphs, problems, dashboards, wall displays, the Git sync state and manage users and enrolment keys.

**Architecture:** M4 is split into three plans: this one (the API, in Rust), M4b (the web UI for the desk, embedded in the server binary) and M4c (wall displays in the browser). This plan adds a second router next to the agent API under `/api/v1`, with cookie sessions stored in PostgreSQL and role checks done by request extractors, so a handler cannot be written without stating the role it needs. Read models (host state, dashboard data, a display's alarm target) are pure functions over rows the store loads in few queries.

**Tech Stack:** Rust 2024, axum, sqlx/PostgreSQL, argon2 (password hashing), the existing `MetricsStore` (VictoriaMetrics).

**Spec:** `docs/superpowers/specs/2026-11-14-cerbrix-design.md` (sections 5.9, 5.10) and `docs/superpowers/specs/2026-11-15-ui-design-concept.md` (sections 5, 6, 8). Issues #31 (API part), #32, #65 (server part), #66, and the server side of #33–#37, #63, #64.

## Global Constraints

- License Apache-2.0; English for code, comments, documentation and commit messages.
- `unsafe_code = "forbid"`. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace` and `cargo test --workspace` pass after every task.
- Every endpoint under `/api/v1` states the role it requires through its extractor; there is no handler without one except login. Roles: `viewer` (read), `operator` (also acknowledge and close problems), `admin` (also users, enrolment keys, host bindings, display tokens).
- No server state only in process memory: users, sessions and display tokens live in PostgreSQL.
- Passwords are stored only as Argon2id hashes; session tokens and display tokens only as SHA-256 hashes. No secret appears in logs, error messages, `Debug` output or API responses, except a newly created token or generated password, which is returned exactly once.
- With GitOps on, hosts, templates, notifications, dashboards and displays are read-only in the API.
- Text that originates in configuration files or operator input is returned as data (JSON strings); the API never builds HTML.
- The agent API (`/api/agent/v1/*`, `/healthz`) keeps its behaviour and its authentication.
- Times in API responses are RFC 3339 in UTC. Errors are `{"error": "<code>", "message": "<text>"}` with stable codes.
- Tests that wait poll for a condition with a bound of 30 s; none asserts right after a fixed sleep. Database tests use `#[sqlx::test(migrations = "./migrations")]`.

## Decisions this plan makes

| # | Decision | Why |
|---|---|---|
| B1 | M4 is three plans: API and login (this), desk UI (M4b), wall displays (M4c) | Each yields working, testable software; the UI plans need the API's shapes fixed first |
| B2 | Login uses a session cookie (`HttpOnly`, `SameSite=Strict`, `Secure` by default when the server itself serves HTTPS, switchable for reverse proxies), sessions in PostgreSQL; idle limit 12 hours, absolute limit 7 days | A browser UI served from the same origin; sessions can be revoked and survive a restart |
| B3 | Passwords: Argon2id, at least 12 and at most 256 characters, no composition rules | Length matters more than character classes; the upper bound caps hashing cost |
| B4 | Five failed logins in a row lock a user for 5 minutes; a wrong user name and a wrong password give the same answer | Slows guessing without a way to lock an account for ever |
| B5 | The first admin is created with `cerbrix-server admin user create <name> --role admin`; the generated password is printed once | #32 "at installation"; same pattern as enrolment keys |
| B6 | The last enabled admin cannot be deleted, disabled or demoted | Otherwise an installation can lock itself out |
| B7 | Changing a user's password or role, or disabling them, ends that user's sessions | A stolen session must not outlive the change |
| B8 | State-changing requests must come from the same origin (checked through `Origin` / `Sec-Fetch-Site`) and carry JSON | Cross-site request forgery protection without a token to manage |
| B9 | The UI gets fresh data by asking again (every 5 seconds on a wall); no push channel in this plan | Meets "a new alarm within about 5 seconds" with the least machinery |
| B10 | One request returns everything a dashboard shows (hosts, problems, graphs, values) | A wall display then needs one permission check and one round trip |
| B11 | A display has at most one token; creating a new one replaces the old | Simple to reason about; a monitor is one device |
| B12 | Operators may close problems by hand as well as acknowledge them | Closing is the only way to end an alarm for something that no longer exists |
| B13 | When the metrics store is unreachable, lists still answer without current values; only graph requests fail | A dashboard should not go blank because graphs are unavailable |
| B14 | The API is documented by hand in `docs/api.md`, and a test fails when a route is missing from it | Smallest thing that keeps documentation and routes together |
| B15 | Problems record whether they come from a comparison or a no-data trigger (new column) | The host list filters by "no data", and the UI words the two differently |

## Review Focus

1. Every route, tried with no session, with each role, and with a display token: only what the role allows answers 2xx. → Task 2 builds the matrix test; every later task adds its routes to it.
2. A user is disabled, deleted or has their password changed while logged in: their next request is 401. A session past its idle or absolute limit is 401. → Tasks 2, 3.
3. A display token reads only the dashboards its display may show, and no host, problem or series outside them; a token of display A is refused on display B. → Task 9.
4. Search and filter input containing `%`, `_`, quotes or very long text neither matches too much nor fails. → Tasks 4, 6.
5. 5,000 hosts and 10,000 problems: list endpoints page, and no handler runs a query per row. → Tasks 4, 6, 8.

## File Structure

```
crates/cerbrix-server/src/
  auth/mod.rs        roles, password hashing, session cookie, extractors, origin check
  state/users.rs     users, sessions
  state/displays.rs  display tokens
  state/read.rs      list and detail queries for hosts, problems, dashboards
  view/mod.rs        HostState and its rule
  view/dashboard.rs  dashboard data assembled from rows
  view/display.rs    a display's alarm target (pure)
  api/ui/mod.rs      the /api/v1 router, paging and error helpers
  api/ui/session.rs  login, logout, own password
  api/ui/users.rs    api/ui/hosts.rs    api/ui/problems.rs
  api/ui/config.rs   status, sync, unknown agents, enrolment keys
  api/ui/dashboards.rs   api/ui/displays.rs
crates/cerbrix-server/migrations/0004_m4a.sql
docs/api.md
```

Shared API conventions (defined in Task 2, used by all later tasks):

- Lists take `limit` (default 50, 1..=500) and `offset` (default 0) and answer `{"items": [...], "total": N}`.
- Error codes: `unauthorized` 401, `forbidden` 403, `not_found` 404, `invalid` 400 (the message names the field), `conflict` 409, `too_many_attempts` 429, `managed_by_git` 409, `metrics_unavailable` 503, `internal` 500.
- Request bodies are JSON, at most 64 KiB.

---

### Task 1: Users, password hashing and the admin commands

**Files:**
- Create: `crates/cerbrix-server/migrations/0004_m4a.sql`, `src/auth/mod.rs`, `src/state/users.rs`
- Modify: `src/lib.rs`, `src/state/mod.rs`, `src/admin.rs`, `src/main.rs`, `docs/install.md`

**Interfaces:**
- Produces:

```rust
// auth/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]   // Viewer < Operator < Admin
pub enum Role { Viewer, Operator, Admin }                     // as_str / FromStr: "viewer" | "operator" | "admin"
pub const MIN_PASSWORD_CHARS: usize = 12;
pub const MAX_PASSWORD_CHARS: usize = 256;
pub fn validate_user_name(s: &str) -> Result<(), String>;      // ^[a-z0-9][a-z0-9._-]{0,62}$
pub fn validate_password(s: &str) -> Result<(), String>;       // length only, counted in characters
pub async fn hash_password(password: String) -> anyhow::Result<String>;      // Argon2id PHC string, on a blocking thread
pub async fn verify_password(password: String, hash: String) -> bool;        // on a blocking thread; false for a malformed hash
pub fn generate_password() -> String;                          // 20 characters from a-z A-Z 0-9

// state/users.rs
pub struct User { pub id: Uuid, pub name: String, pub role: Role, pub disabled: bool,
    pub created_at: OffsetDateTime, pub password_changed_at: OffsetDateTime }
impl StateStore {
    pub async fn create_user(&self, name: &str, role: Role, password_hash: &str, now: OffsetDateTime) -> Result<User, StateError>; // Conflict on a taken name
    pub async fn users(&self) -> Result<Vec<User>, StateError>;                 // by name
    pub async fn user_by_name(&self, name: &str) -> Result<Option<User>, StateError>;
    pub async fn set_user_role(&self, id: Uuid, role: Role) -> Result<User, StateError>;
    pub async fn set_user_disabled(&self, id: Uuid, disabled: bool) -> Result<User, StateError>;
    pub async fn set_user_password(&self, id: Uuid, password_hash: &str, now: OffsetDateTime) -> Result<(), StateError>;
    pub async fn delete_user(&self, id: Uuid) -> Result<(), StateError>;
}
```

Migration `0004_m4a.sql` (all tables of this plan, so later tasks do not add migrations):

```sql
CREATE TABLE users (
    id                  uuid PRIMARY KEY,
    name                text NOT NULL UNIQUE,
    role                text NOT NULL CHECK (role IN ('viewer', 'operator', 'admin')),
    password_hash       text NOT NULL,
    disabled            boolean NOT NULL DEFAULT false,
    failed_logins       integer NOT NULL DEFAULT 0,
    locked_until        timestamptz,
    created_at          timestamptz NOT NULL,
    password_changed_at timestamptz NOT NULL
);

CREATE TABLE sessions (
    token_hash   bytea PRIMARY KEY,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    expires_at   timestamptz NOT NULL
);
CREATE INDEX sessions_user ON sessions (user_id);

CREATE TABLE display_tokens (
    display_id text PRIMARY KEY,
    token_hash bytea NOT NULL UNIQUE,
    created_at timestamptz NOT NULL
);

ALTER TABLE problems ADD COLUMN kind text NOT NULL DEFAULT 'compare' CHECK (kind IN ('compare', 'no_data'));
CREATE INDEX problems_open_host ON problems (host_id) WHERE resolved_at IS NULL;
```

- `set_user_role`, `set_user_disabled(true)` and `delete_user` refuse with `Conflict("the last enabled admin cannot be changed")` when the user is the only enabled admin; the check and the change are one transaction that locks the admin rows. All three, and `set_user_password`, delete the user's sessions in the same transaction.
- Admin commands (no HTTP): `admin user create <NAME> --role <ROLE> [--password-stdin]` prints `user <name> created with role <role>` and, without `--password-stdin`, a second line `password: <generated>` (shown once; with the flag the password is the first line of standard input and is validated). `admin user list` prints one line per user: name, role, `enabled`/`disabled`, creation time. `admin user set-role <NAME> <ROLE>`, `admin user disable|enable <NAME>`, `admin user reset-password <NAME> [--password-stdin]`, `admin user delete <NAME>`; each prints one line saying what happened.
- `docs/install.md`: a short "First login" section with the create command.

- [ ] **Step 1: Write the failing tests**

  - `role_order_and_text`: `Viewer < Operator < Admin`; round trip of the three texts; `"root"` fails.
  - `user_name_rules`: accepts `t.marzec`, `ops-1`; rejects `""`, `Admin`, `-a`, 64 characters, `a b`.
  - `password_rules`: 11 characters rejected, 12 accepted, 256 accepted, 257 rejected; counted in characters (12 umlauts are accepted).
  - `hash_and_verify`: the right password verifies, a wrong one does not; two hashes of one password differ; the hash starts with `$argon2id$`; a malformed hash verifies as false without panic.
  - `generated_password_shape`: 20 characters, alphanumeric, two calls differ.
  - Store: `create_and_find_user`; `duplicate_name_conflicts`; `last_admin_is_protected` (demote, disable and delete each refuse; with a second enabled admin each succeeds); `concurrent_removal_of_two_admins_keeps_one` (two tasks each demoting a different admin of two → exactly one succeeds); `password_change_ends_sessions` (insert a session row by hand, change the password → no session rows).
  - Admin: `user_create_prints_password_once` (the password line is present, verifies against the stored hash, and the stored hash is not the password); `user_create_with_stdin_password_prints_no_password`; `user_list_format`; `set_role_disable_delete_messages`; `unknown_user_is_an_error`.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server auth state::users admin` — expected: fails to compile.
- [ ] **Step 3: Implement.** Add `argon2` with default parameters; hashing and verifying run in `tokio::task::spawn_blocking`.
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): users, password hashing and admin user commands`

---

### Task 2: Sessions, login and role enforcement

**Files:**
- Create: `src/api/ui/mod.rs`, `src/api/ui/session.rs`
- Modify: `src/auth/mod.rs`, `src/state/users.rs`, `src/api/mod.rs`, `src/config.rs`

**Interfaces:**
- Consumes: Task 1.
- Produces:

```rust
// config.rs — Config gains `#[serde(default)] pub auth: AuthConfig`
pub struct AuthConfig { pub session_idle_secs: u64 /* default 43200, 300..=604800 */,
                        pub session_max_secs: u64  /* default 604800, >= idle, <= 2592000 */,
                        pub secure_cookie: Option<bool> /* default: !server.insecure_http */ }

// state/users.rs
pub enum LoginOutcome { Ok(User, String /* session token */), Wrong, Locked }
impl StateStore {
    pub async fn login(&self, name: &str, password: &str, now: OffsetDateTime, cfg: &AuthConfig) -> Result<LoginOutcome, StateError>;
    pub async fn session_user(&self, token: &str, now: OffsetDateTime, cfg: &AuthConfig) -> Result<Option<User>, StateError>;
    pub async fn logout(&self, token: &str) -> Result<(), StateError>;
    pub async fn delete_expired_sessions(&self, now: OffsetDateTime) -> Result<u64, StateError>;
}

// auth/mod.rs — extractors for AppState; each rejects with 401 `unauthorized` or 403 `forbidden`
pub struct Viewer(pub User);  pub struct Operator(pub User);  pub struct Admin(pub User);
pub const SESSION_COOKIE: &str = "cerbrix_session";

// api/ui/mod.rs
pub fn router(state: AppState) -> Router;        // everything under /api/v1, merged into api::router
pub struct Page { pub limit: i64, pub offset: i64 }           // query extractor with the bounds above
pub struct Listed<T> { pub items: Vec<T>, pub total: i64 }    // Serialize
pub(crate) const ROUTES: &[(&str, &str, Access)];             // method, path, access; one line per route
pub(crate) enum Access { Public, Viewer, Operator, Admin, Display }
```

Routes of this task:

| Method and path | Access | Behaviour |
|---|---|---|
| `POST /api/v1/session` | public | body `{"name", "password"}` → 200 `{"name", "role"}` and the cookie; wrong → 401 `unauthorized` "wrong user name or password"; locked → 429 `too_many_attempts` |
| `GET /api/v1/session` | viewer | `{"name", "role"}` |
| `DELETE /api/v1/session` | viewer | 204, cookie cleared, session row deleted |
| `PUT /api/v1/session/password` | viewer | body `{"current", "new"}` → 204; wrong current → 403; invalid new → 400; the user's other sessions end, this one stays |

- `login`: an unknown or disabled user and a wrong password give `Wrong`, and take the same time (a dummy hash is verified when there is no user). Five consecutive wrong passwords set `locked_until = now + 5 min`; a locked user gets `Locked` without a password check; a successful login resets the counter. The session token is `tokens::generate("cbx_se_")`; only its SHA-256 is stored; `expires_at = now + session_max_secs`.
- `session_user`: `None` for an unknown token, a disabled user, `now >= expires_at`, or `now - last_seen_at > session_idle_secs`; otherwise the user, and `last_seen_at` is set to `now` when it is older than 60 s.
- Cookie: `cerbrix_session=<token>; Path=/; HttpOnly; SameSite=Strict; Max-Age=<session_max_secs>` plus `Secure` when `auth.secure_cookie` is true. That setting defaults to the opposite of `server.insecure_http`; behind a TLS-terminating reverse proxy the operator sets it to true.
- Same-origin rule for every method other than GET and HEAD under `/api/v1` (a middleware): if `Origin` is present its host and port must equal the request's `Host` header (the scheme is not compared, because a TLS-terminating reverse proxy changes it); else if `Sec-Fetch-Site` is present it must be `same-origin`; otherwise the request is let through (non-browser clients). A violation is 403 `forbidden`. A body must have `Content-Type: application/json`, else 400 `invalid`.
- Responses under `/api/v1` carry `Cache-Control: no-store`.
- A background task in `serve` deletes expired sessions every 10 minutes (loop written and tested like the others).
- `ROUTES` lists every `/api/v1` route with its access level; `router` is built from the same list or a test proves the two agree.

- [ ] **Step 1: Write the failing tests**

  - Store: `login_ok_creates_a_session`; `wrong_password_and_unknown_user_look_the_same` (both `Wrong`); `five_failures_lock_for_five_minutes` (the sixth attempt with the right password is `Locked`; after 5 minutes it is `Ok`); `success_resets_the_failure_count`; `disabled_user_cannot_log_in`; `session_expires_idle_and_absolute`; `last_seen_is_refreshed_at_most_once_a_minute`; `logout_ends_the_session`; `only_the_hash_is_stored` (the token text appears in no column).
  - Router: `login_sets_the_cookie_with_its_attributes` (with and without `insecure_http`, and with `auth.secure_cookie = true` under `insecure_http`); `origin_with_other_scheme_but_same_host_passes`; `session_endpoint_needs_a_session`; `logout_clears_cookie_and_session`; `change_own_password` (old password no longer logs in, new one does, a second session of the user is ended, the current one works); `cross_origin_post_is_forbidden` (an `Origin` of another host; `Sec-Fetch-Site: cross-site`); `same_origin_post_passes`; `post_without_json_content_type_is_invalid`; `responses_are_not_cacheable`; `body_over_64_kib_is_rejected`.
  - `access_matrix`: a table-driven test over `ROUTES` that calls every route with no credentials, as viewer, as operator and as admin (and, from Task 9 on, with a display token), and asserts: below the required level → 401 without credentials, 403 with too low a role; at or above → not 401 and not 403. Path parameters are filled with ids that exist in a fixture. This test is extended, not copied, by every later task.
  - `routes_and_router_agree`: every path the router serves under `/api/v1` is in `ROUTES` and the reverse.
  - Config: `auth_defaults_and_bounds` (idle 299 rejected; max below idle rejected).
  - Loop: `expired_sessions_are_deleted` (polling with a bound).

- [ ] **Step 2: Run** `cargo test -p cerbrix-server auth api::ui state::users config` — expected: fails to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p cerbrix-server` — expected: all pass.
- [ ] **Step 5: Commit** `feat(server): sessions, login and role enforcement`

---

### Task 3: Users API

**Files:**
- Create: `src/api/ui/users.rs`
- Modify: `src/api/ui/mod.rs`

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces (all admin):

| Method and path | Behaviour |
|---|---|
| `GET /api/v1/users` | `Listed` of `{"id", "name", "role", "disabled", "created_at", "password_changed_at"}` |
| `POST /api/v1/users` | body `{"name", "role", "password"?}` → 201 with the user and, when no password was given, `"password": "<generated>"` once; taken name → 409 `conflict`; invalid name, role or password → 400 |
| `PATCH /api/v1/users/{id}` | body with any of `"role"`, `"disabled"` → 200 with the user; the last enabled admin → 409 `conflict` |
| `POST /api/v1/users/{id}/password` | body `{"password"?}` → 200 `{}` or `{"password": "<generated>"}` |
| `DELETE /api/v1/users/{id}` | 204; the last enabled admin → 409; deleting oneself is allowed unless one is the last admin |

No response ever contains a password hash.

- [ ] **Step 1: Write the failing tests**

  - `create_user_with_generated_password`: 201, the returned password logs in, a second `GET` does not show it.
  - `create_user_validation`: bad name, unknown role, short password → 400 naming the field; duplicate → 409.
  - `patch_role_and_disable`; `disabled_user_is_logged_out` (their existing session gets 401 on the next request).
  - `reset_password_ends_sessions`.
  - `last_admin_cannot_be_removed_through_the_api` (PATCH role, PATCH disabled, DELETE → 409).
  - `unknown_id_is_not_found`; `no_hash_in_any_response` (the JSON of every endpoint here contains neither `argon2` nor `password_hash`).
  - Extend `access_matrix` with these routes.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server api::ui` — expected: the new tests fail with 404.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): users API`

---

### Task 4: Host state, hosts list and host detail

**Files:**
- Create: `src/view/mod.rs`, `src/state/read.rs`, `src/api/ui/hosts.rs`
- Modify: `src/lib.rs`, `src/alerting/mod.rs`, `src/alerting/eval.rs`, `src/alerting/nodata.rs`, `src/state/alerting.rs` (the `kind` column), `src/api/ui/mod.rs`

**Interfaces:**
- Consumes: the tables of M2 and M3.
- Produces:

```rust
// alerting/mod.rs
pub enum ProblemKind { Compare, NoData }          // as_str(): "compare" | "no_data"
// OpenProblem gains `pub kind: ProblemKind`; evaluate sets Compare, evaluate_nodata sets NoData; the store writes it.
// state::alerting::Problem gains `pub kind: ProblemKind`.

// view/mod.rs
#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct HostState { pub severity: Severity, pub acknowledged: bool, pub kind: ProblemKind,
    pub trigger: String, pub instance: String, pub summary: String, pub open_problems: usize }
/// None when the host has no open problem.
pub fn host_state(open_problems_of_host: &[Problem]) -> Option<HostState>;

// state/read.rs
pub enum HostStateFilter { WithProblems, NoData, Ok }
pub struct HostQuery { pub search: Option<String>, pub group: Option<String>,
    pub state: Option<HostStateFilter>, pub limit: i64, pub offset: i64 }
pub struct HostRow { pub id: Uuid, pub name: String, pub address: Option<String>, pub groups: Vec<String>,
    pub source_file: Option<String>, pub last_seen_at: Option<OffsetDateTime>, pub open: Vec<Problem> }
impl StateStore {
    pub async fn hosts_page(&self, q: &HostQuery) -> Result<(Vec<HostRow>, i64), StateError>;
    pub async fn host_groups(&self) -> Result<Vec<String>, StateError>;          // distinct, sorted
    pub async fn host_row(&self, name: &str) -> Result<Option<HostRow>, StateError>;
}
```

`host_state` rule (UI concept 5.1): among the open problems, an unacknowledged one outranks an acknowledged one; then the higher severity; then the older. The result describes that problem; `acknowledged` is true only when every open problem of the host is acknowledged.

`hosts_page`: active hosts only. `search` matches name or address as a case-insensitive substring, with `%`, `_` and `\` in the input taken literally. Order: hosts with an unacknowledged open problem first, most severe first, then hosts with only acknowledged problems, then by name. `state`: `WithProblems` = at least one open problem; `NoData` = at least one open problem of kind `no_data`; `Ok` = none. The page and the open problems of its hosts are loaded in two queries, not one per host.

Routes (viewer):

| Method and path | Answer |
|---|---|
| `GET /api/v1/hosts?search&group&state&limit&offset` | `Listed` of `{"name", "address", "groups", "state": HostState or null, "last_seen_at", "managed_by_git": bool}` |
| `GET /api/v1/host-groups` | `{"items": ["db", "production", …]}` |
| `GET /api/v1/hosts/{name}` | `{"name", "address", "groups", "templates", "source_file", "managed_by_git", "enrolled_at": time or null, "last_seen_at", "state", "items": [{"key", "params", "interval_secs", "origin"}], "triggers": [{"name", "expression", "severity", "origin", "firing": bool}], "open_problems": [Problem]}`; unknown or inactive → 404 |

A problem in JSON: `{"id", "host", "trigger", "instance", "severity", "kind", "summary", "started_at", "resolved_at", "resolve_reason", "acknowledged_at", "acknowledged_by", "ack_comment"}`. `firing` is true when any `trigger_states` row of that trigger is firing. `state` values of the query parameter: `problems`, `no_data`, `ok`; anything else → 400.

- [ ] **Step 1: Write the failing tests**

  - `host_state_rule`: no problems → `None`; unacknowledged warning beats acknowledged critical; among unacknowledged, critical beats high; equal severity → older; `acknowledged` true only when all are; `open_problems` counts all.
  - `problems_record_their_kind`: a comparison problem is `compare`, a no-data problem `no_data` (through `evaluate_host` with each evaluator).
  - Store: `hosts_page_orders_problems_first`; `search_is_literal` (a host named `a_b` is found by `a_b`, and `a_b` does not match `axb`; `%` matches nothing but itself); `filter_by_group_and_state`; `paging_and_total`; `inactive_hosts_are_not_listed`; `page_of_200_hosts_with_problems_is_correct` (200 hosts with 3 open problems each: every host has exactly its own problems).
  - Router: `hosts_list_shape`; `host_detail_shape` (the example repository's `web-01`: `cpu.usage` at 10 s with origin `host`, a trigger with `firing` true after a firing value); `unknown_host_is_not_found`; `bad_state_parameter_is_invalid`; `managed_by_git_follows_the_setting`.
  - Extend `access_matrix`.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server view state::read api::ui alerting` — expected: fails to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p cerbrix-server` — expected: all pass (M3 tests adapted to the new field).
- [ ] **Step 5: Commit** `feat(server): host state and hosts API`

---

### Task 5: Current values and history graphs

**Files:**
- Modify: `src/metrics/mod.rs`, `src/metrics/victoria.rs`, `src/metrics/memory.rs`, `src/api/ui/hosts.rs`, `crates/cerbrix-common/src/items.rs`

**Interfaces:**
- Consumes: `MetricsStore`, `BUILTIN_ITEMS`.
- Produces:

```rust
// cerbrix-common items.rs
pub fn metric_name(key: &str) -> String;                       // "cpu.usage" → "cerbrix_cpu_usage" (as the store names series today; move the existing function here if it lives in the server)
pub fn key_for_metric(name: &str) -> Option<&'static str>;     // reverse lookup through BUILTIN_ITEMS

// metrics/mod.rs — MetricsStore gains
async fn latest(&self, query: &LatestQuery) -> Result<Vec<LatestSample>, MetricsError>;
pub struct LatestQuery { pub host: Option<String>, pub key: Option<String>, pub max_age_secs: u32 }
pub struct LatestSample { pub host: String, pub key: String, pub params: BTreeMap<String, String>,
                          pub ts_ms: i64, pub value: f64 }
```

- `latest` returns the newest sample of every matching series that is not older than `max_age_secs`. Series whose metric name is not a built-in item are skipped. The VictoriaMetrics implementation uses one instant query per call; host and key are validated before they go into the query, as `query_range` does.
- Hosts list (Task 4's route) gains `"cpu": number or null` (`cpu.usage`) and `"memory": number or null` (`memory.used_percent`) per host and a top-level `"metrics_available": bool`: two `latest` calls for the whole page's hosts (one per key, `max_age_secs` 600); when the store is unavailable the list still answers with nulls and `false`.

Routes (viewer):

| Method and path | Answer |
|---|---|
| `GET /api/v1/hosts/{name}/values` | `{"items": [{"key", "params", "value", "at"}], "metrics_available": bool}`, sorted by key then params; `max_age_secs` 3600 |
| `GET /api/v1/hosts/{name}/series?item=<key>&range=<1h\|6h\|24h\|7d>&<p_name>=<value>…` | `{"key", "params", "from", "to", "step_secs", "points": [[ts_ms, value], …], "thresholds": [{"trigger", "severity", "op", "value"}]}`; store unavailable → 503 `metrics_unavailable` |

- Item parameters are passed as query parameters prefixed `p_` (`p_mount=/`). The item key must be one of the host's effective items, else 400. Steps: `1h` → 15 s, `6h` → 90 s, `24h` → 300 s, `7d` → 1800 s. `thresholds` lists every comparison trigger of the host on that item whose selector matches the requested parameters.

- [ ] **Step 1: Write the failing tests**

  - `metric_name_round_trips_for_every_builtin_item`; `key_for_unknown_metric_is_none`.
  - Memory store: `latest_returns_newest_per_series`; `latest_respects_max_age`; `latest_filters_by_host_and_key`.
  - VictoriaMetrics (wiremock, as the existing tests): `latest_builds_one_instant_query` (asserts the query string for host+key, host only, key only); `latest_maps_labels_to_params` (`p_mount` → `mount`); `latest_skips_unknown_metric_names`; `latest_unavailable_and_rejected_mapping`; `latest_rejects_bad_host_or_key_before_any_request`.
  - Router: `hosts_list_has_cpu_and_memory`; `hosts_list_survives_metrics_outage` (a metrics store that fails → 200, nulls, `metrics_available` false); `values_shape_and_order`; `series_returns_points_and_thresholds` (the `disk.used_percent > 90` trigger gives one threshold `{"op": ">", "value": 90}`); `series_step_per_range`; `series_unknown_item_is_invalid`; `series_bad_range_is_invalid`; `series_metrics_outage_is_503`.
  - The ignored real-VictoriaMetrics test gains a `latest` round trip.
  - Extend `access_matrix`.

- [ ] **Step 2: Run** `cargo test -p cerbrix-common items && cargo test -p cerbrix-server metrics api::ui` — expected: fails to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests, and the ignored test once against the dev VictoriaMetrics (`cargo test -p cerbrix-server -- --ignored`) — expected: pass.
- [ ] **Step 5: Commit** `feat(server): current values and history series`

---

### Task 6: Problems API

**Files:**
- Create: `src/api/ui/problems.rs`
- Modify: `src/state/read.rs`, `src/api/ui/mod.rs`

**Interfaces:**
- Consumes: `Problem`, `acknowledge_problem`, `close_problem` (M3), `Operator`.
- Produces:

```rust
pub enum ProblemStateFilter { Open, Resolved, All }
pub struct ProblemQuery { pub state: ProblemStateFilter, pub min_severity: Option<Severity>,
    pub host: Option<String>, pub group: Option<String>, pub dashboard: Option<String>,
    pub search: Option<String>, pub limit: i64, pub offset: i64 }
impl StateStore {
    pub async fn problems_page(&self, q: &ProblemQuery, dashboard_hosts: Option<&[Uuid]>) -> Result<(Vec<Problem>, i64), StateError>;
    pub async fn problem(&self, id: Uuid) -> Result<Option<Problem>, StateError>;
    pub async fn open_problem_counts(&self) -> Result<ProblemCounts, StateError>;   // per severity: total and unacknowledged
}
```

| Method and path | Access | Behaviour |
|---|---|---|
| `GET /api/v1/problems?state&severity&host&group&dashboard&search&limit&offset` | viewer | `Listed` of problems. `state`: `open` (default), `resolved`, `all`. `severity` is a minimum. `dashboard` limits to the hosts that dashboard selects. `search` matches host, trigger or summary, literal. Order: open → unacknowledged first, most severe first, oldest first; resolved and all → newest start first |
| `GET /api/v1/problems/{id}` | viewer | the problem, or 404 |
| `POST /api/v1/problems/{id}/ack` | operator | body `{"comment"}` (may be empty) → 200 with the problem; `acknowledged_by` is the session's user name; a resolved problem → 409 `conflict` |
| `POST /api/v1/problems/{id}/close` | operator | body `{"comment"}` (required, not blank) → 200 with the problem; already resolved → 409 |

Comments are cut to 1,000 characters and control characters are replaced by spaces, as the admin commands do; reuse that code.

- [ ] **Step 1: Write the failing tests**

  - Store: `open_problems_order`; `filter_by_min_severity_host_group`; `filter_by_dashboard_hosts`; `search_is_literal_across_host_trigger_summary`; `resolved_are_newest_first`; `paging_and_total`; `counts_per_severity`.
  - Router: `list_defaults_to_open`; `bad_state_or_severity_is_invalid`; `unknown_dashboard_is_invalid`; `ack_records_the_session_user`; `ack_by_viewer_is_forbidden`; `ack_of_resolved_is_conflict`; `close_needs_a_comment`; `close_resolves_and_queues_mail_when_configured` (with `queue_mail` true an outbox row for the end exists); `closed_problem_reopens_on_the_next_firing_value`.
  - Extend `access_matrix`.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server state::read api::ui` — expected: fails to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): problems API`

---

### Task 7: Status, sync, unknown agents and enrolment keys

**Files:**
- Create: `src/api/ui/config.rs`
- Modify: `src/state/enrollment.rs`, `src/state/read.rs`, `src/api/ui/mod.rs`

**Interfaces:**
- Consumes: `sync_state`, `sync_history`, `unknown_agents`, `create_enrollment_key`, `revoke_enrollment_key`, `release_binding`, `open_problem_counts`.
- Produces: `StateStore::enrollment_keys() -> Vec<EnrollmentKey { id, description, created_at, expires_at, revoked_at }>` (newest first) and `host_counts() -> HostCounts { total, with_problems, without_problems, never_seen }`.

| Method and path | Access | Answer |
|---|---|---|
| `GET /api/v1/status` | viewer | `{"version", "gitops": bool, "mail": bool, "sync": {"status": "never"\|"in_sync"\|"out_of_sync", "active_commit", "last_success_at", "last_attempt_at", "issues": [{"file", "line", "column", "reason"}]}, "hosts": HostCounts, "problems": {"critical": {"open", "unacknowledged"}, "high": …, "warning": …, "info": …}}` |
| `GET /api/v1/sync/history?limit&offset` | viewer | `Listed` of `{"commit", "applied_at", "changes": {"hosts": {"added", "changed", "removed"}, …}}` |
| `GET /api/v1/unknown-agents` | operator | `{"items": [{"host_name", "first_seen", "last_seen", "attempts"}]}` |
| `GET /api/v1/enrollment-keys` | admin | `{"items": [EnrollmentKey + "state": "active"\|"expired"\|"revoked"]}`; never the key |
| `POST /api/v1/enrollment-keys` | admin | body `{"description", "expires_in"}` (the interval syntax, 1 h to 365 d) → 201 with the key record and `"key": "<plain>"` once |
| `DELETE /api/v1/enrollment-keys/{id}` | admin | 204 (revokes; unknown id → 404) |
| `POST /api/v1/hosts/{name}/release` | admin | 204: the host's agent binding is removed so it can enrol again; unknown host → 404 |

`/status` is what a page header needs on every refresh: keep it to three queries.

- [ ] **Step 1: Write the failing tests**

  - `status_shape_in_sync_and_out_of_sync` (after a failing sync the issues carry file, line and reason); `status_counts` (hosts and problems per severity with unacknowledged counts); `status_reports_gitops_and_mail_flags`.
  - `sync_history_pages`.
  - `unknown_agents_listed_for_operator_not_viewer`.
  - `create_key_returns_it_once` (the list afterwards has no `key` field and no value resembling it; the returned key enrols an agent through the agent API); `key_expiry_bounds` (59 minutes and 366 days → 400); `revoked_and_expired_states`; `revoke_unknown_is_not_found`.
  - `release_lets_the_host_enrol_again`.
  - Extend `access_matrix`.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server api::ui state` — expected: fails to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): status, sync history, unknown agents and enrolment keys API`

---

### Task 8: Dashboards API

**Files:**
- Create: `src/view/dashboard.rs`, `src/api/ui/dashboards.rs`
- Modify: `src/state/read.rs`, `src/api/ui/mod.rs`

**Interfaces:**
- Consumes: `DashboardFile`, `Panel`, `HostSelector::selects`, `effective_panels` (M2), `host_state`, `latest`, `query_range`.
- Produces:

```rust
// state/read.rs
pub struct BoardInputs { pub dashboards: Vec<(String /*id*/, String /*source file*/, DashboardFile)>,
    pub hosts: Vec<HostRow> /* all active hosts with their open problems */ }
impl StateStore { pub async fn board_inputs(&self) -> Result<BoardInputs, StateError>; }

// view/dashboard.rs
#[derive(Serialize)] pub struct DashboardSummary { pub id: String, pub name: String, pub file: String,
    pub priority: i32, pub host_count: usize, pub hosts_by_state: StateCounts,
    pub open_problems: SeverityCounts, pub unacknowledged: SeverityCounts }
pub fn summaries(inputs: &BoardInputs) -> Vec<DashboardSummary>;          // sorted as below
pub fn selected_hosts<'a>(d: &DashboardFile, hosts: &'a [HostRow]) -> Vec<&'a HostRow>;
```

`summaries` order: dashboards with an unacknowledged open problem first; among those the higher `priority`, then the more severe unacknowledged problem, then the older one; the rest by `priority` descending, then name.

| Method and path | Access | Answer |
|---|---|---|
| `GET /api/v1/dashboards` | viewer | `{"items": [DashboardSummary]}` |
| `GET /api/v1/dashboards/{id}` | viewer | `{"id", "name", "file", "priority", "hosts": [{"name", "state"}], "panels": [panel], "metrics_available": bool}`; unknown → 404 |

A panel in the answer has `type`, `x`, `y`, `w`, `h` and its data:
- `problems`: `"problems": [Problem]` — the open problems of the selected hosts, unacknowledged first, most severe first, oldest first, at most 100, plus `"more": N`.
- `host-states`: `"total"`, `"without_problems"`, `"hosts": [{"name", "state"}]` (problem hosts first).
- `graph`: `"host"`, `"item"`, `"params"`, `"range"`, `"step_secs"`, `"points"`, `"thresholds"` as in Task 5 (steps chosen so a range yields at most about 300 points); when the metrics store is unavailable `"points": null`.
- `value`: `"host"`, `"item"`, `"params"`, `"value"`, `"at"` (null when there is none).

All graph and value panels of one dashboard are fetched concurrently, with the whole answer bounded by the request timeout; one failing series makes that panel's data null, not the request fail.

- [ ] **Step 1: Write the failing tests**

  - `summaries_count_hosts_and_problems`; `summaries_order_follows_alarms_then_priority`; `selector_semantics_reused` (a host in all groups; a named host; the empty selector selects all).
  - Router: `dashboards_list_shape` (the example repository's `production`); `dashboard_detail_standard_layout` (a dashboard without panels answers the two standard panels); `problems_panel_caps_at_100_and_reports_more`; `graph_panel_has_points_and_thresholds`; `value_panel_latest`; `metrics_outage_gives_null_data_not_an_error`; `unknown_dashboard_is_not_found`.
  - `dashboard_with_many_hosts`: 1,000 hosts, 2,000 open problems → correct counts (volume correctness).
  - Extend `access_matrix`.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server view api::ui` — expected: fails to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests — expected: pass.
- [ ] **Step 5: Commit** `feat(server): dashboards API`

---

### Task 9: Wall displays: alarm target, API and display tokens

**Files:**
- Create: `src/view/display.rs`, `src/state/displays.rs`, `src/api/ui/displays.rs`
- Modify: `src/auth/mod.rs`, `src/state/sync.rs`, `src/api/ui/mod.rs`

**Interfaces:**
- Consumes: `DisplayFile`, `Show`, `SwitchOnAlarm` (M2), `BoardInputs`, the dashboard answer of Task 8.
- Produces:

```rust
// view/display.rs
#[derive(Serialize, PartialEq, Debug)]
pub struct DisplayTarget { pub target: Option<String>, pub causes: Vec<Uuid>, pub waiting: Vec<String> }
/// Dashboards the display may ever show: its `show` targets plus its switch targets.
pub fn allowed_dashboards(display: &DisplayFile, all_dashboards: &[String]) -> Vec<String>;
pub fn alarm_target(display: &DisplayFile, inputs: &BoardInputs) -> DisplayTarget;

// state/displays.rs
impl StateStore {
    pub async fn displays(&self) -> Result<Vec<(String /*id*/, String /*file*/, DisplayFile)>, StateError>;
    pub async fn create_display_token(&self, display_id: &str, now: OffsetDateTime) -> Result<String, StateError>; // replaces an existing one; NotFound for an unknown display
    pub async fn revoke_display_token(&self, display_id: &str) -> Result<(), StateError>;
    pub async fn display_for_token(&self, token: &str) -> Result<Option<String>, StateError>;
    pub async fn display_token_created(&self, display_id: &str) -> Result<Option<OffsetDateTime>, StateError>;
}

// auth/mod.rs
/// A logged-in user of any role, or a display token whose display id equals the `{id}` in the path.
pub struct DisplayAccess { pub display_id: String, pub by_token: bool }
```

`alarm_target` (UI concept section 6):
- No `switch_on_alarm` → `target: None`, nothing waiting.
- An alarm is an open, unacknowledged problem with severity at or above `min_severity` on a host a dashboard selects. Only dashboards in `display.dashboards` (all dashboards when absent) can be targets.
- `target` is the dashboard with an alarm that has the highest `priority`; ties go to the more severe alarm, then the older one, then the dashboard id. `causes` are the ids of that dashboard's alarms, most severe first. `waiting` are the other dashboards with an alarm, in the same order.
- It is a pure function: the same inputs give the same answer on every instance.

Tokens: `tokens::generate("cbx_dt_")`, stored as SHA-256, sent as `Authorization: Bearer <token>`. `apply_config` deletes the tokens of displays that are no longer in the revision, in its transaction.

| Method and path | Access | Answer |
|---|---|---|
| `GET /api/v1/displays` | viewer | `{"items": [{"id", "name", "file", "show", "switch_on_alarm", "dashboards", "token_created_at": time or null, "target", "causes", "waiting"}]}` |
| `POST /api/v1/displays/{id}/token` | admin | 201 `{"token": "<plain>"}` once; unknown display → 404 |
| `DELETE /api/v1/displays/{id}/token` | admin | 204 |
| `GET /api/v1/display/{id}` | display | `{"id", "name", "show", "switch_on_alarm", "allowed": [dashboard ids], "target", "causes", "waiting", "sync": {"status", "active_commit", "issues"}}` |
| `GET /api/v1/display/{id}/dashboards/{dashboard}` | display | the dashboard answer of Task 8; a dashboard outside `allowed` → 403 `forbidden`, for users and tokens alike |

A display token is accepted on the two `display` routes only, and only for its own id; everywhere else it is 401. The same-origin rule does not apply to it (these routes are GET).

- [ ] **Step 1: Write the failing tests**

  - Pure: `no_switch_no_target`; `highest_priority_wins`; `tie_goes_to_more_severe_then_older`; `acknowledged_problems_are_not_alarms`; `below_min_severity_is_not_an_alarm`; `limited_display_never_targets_outside_its_list`; `host_in_two_dashboards_alarms_on_both`; `waiting_lists_the_rest_in_order`; `allowed_is_show_plus_switch_targets` (fixed, rotating, with and without `dashboards`); `deterministic_for_shuffled_input`.
  - Store: `token_is_stored_hashed_and_resolves`; `new_token_replaces_the_old` (the old one no longer resolves); `revoke_ends_it`; `token_for_unknown_display_is_not_found`; `removed_display_loses_its_token` (apply a revision without the display).
  - Router: `displays_list_shape_with_target_and_queue`; `create_token_returns_it_once`; `display_state_with_token`; `token_of_another_display_is_unauthorized`; `token_cannot_read_other_routes` (hosts, problems, dashboards list, users → 401); `dashboard_outside_allowed_is_forbidden` (with a token and as a logged-in viewer); `display_dashboard_data_matches_the_dashboards_route`; `revoked_token_is_unauthorized`.
  - Extend `access_matrix` with the display column.

- [ ] **Step 2: Run** `cargo test -p cerbrix-server view::display state::displays api::ui` — expected: fails to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p cerbrix-server` — expected: all pass.
- [ ] **Step 5: Commit** `feat(server): wall display alarm target, display API and tokens`

---

### Task 10: API documentation and end-to-end walk-through

**Files:**
- Create: `docs/api.md`
- Modify: `scripts/e2e.sh`, `docs/install.md`, `README.md`, `deploy/server.env.example`, `crates/cerbrix-server/src/api/ui/mod.rs` (the documentation test)

**Interfaces:**
- Consumes: everything above.
- Produces: the API reference, a test that keeps it complete, and an end-to-end walk-through with `curl`.

- [ ] **Step 1: Write the failing test** `every_route_is_documented`: for every entry of `ROUTES`, `docs/api.md` contains a heading or table row with its method and path exactly as in `ROUTES`, and the access level stated next to it matches.
- [ ] **Step 2: Extend `scripts/e2e.sh`** (after the alerting steps, with the stack still running):
  1. `admin user create e2e-admin --role admin` and capture the password.
  2. Log in with `curl` (cookie jar); `GET /api/v1/session` shows the role.
  3. `GET /api/v1/status` shows `in_sync` and counts that match `admin problem list`.
  4. `GET /api/v1/hosts` contains the e2e host with a state; `GET /api/v1/hosts/<host>/series?item=cpu.usage&range=1h` has at least one point.
  5. `GET /api/v1/problems` lists the open problem; `POST …/ack` with a comment succeeds and the problem shows the user's name.
  6. `GET /api/v1/dashboards/<id>` for the e2e repository's dashboard has a problems panel with that problem.
  7. `POST /api/v1/displays/<id>/token`, then `GET /api/v1/display/<id>` with the bearer token names the dashboard as target; the same token on `/api/v1/hosts` is 401.
  8. A request without the cookie is 401; a viewer created on the way gets 403 on `POST /api/v1/users`.
  Failure output shows the last HTTP status and body.
- [ ] **Step 3: Write `docs/api.md`:** conventions (base path, JSON, times, errors with every code, paging), login and sessions (cookie, limits, lockout, same-origin rule, how a non-browser client logs in with `curl`), roles and what each may do, display tokens, then every route with method, path, access, parameters, a request example and a response example taken from a real run. State what is not there: no push channel, no write access to configuration with GitOps on, no API versioning promise before 1.0.
- [ ] **Step 4:** `docs/install.md`: first admin, the `[auth]` settings, serving behind a reverse proxy (the `Host` header must reach the server unchanged for the same-origin rule, and `auth.secure_cookie = true` when the proxy terminates TLS). `deploy/server.env.example`: the commented `CERBRIX_AUTH__*` block. `README.md`: one paragraph.
- [ ] **Step 5: Run** `cargo test -p cerbrix-server every_route_is_documented`, `scripts/e2e.sh`, shellcheck and actionlint through their Docker images — expected: pass, exit 0 each.
- [ ] **Step 6: Commit** `feat: API reference and end-to-end API walk-through`
