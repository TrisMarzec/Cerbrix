# Pre-release Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the seven known weaknesses that would hurt in daily operation before a first release: issues #88, #90, #89, #99, #42, #69 and #80.

**Architecture:** Seven independent changes, one task each, in the server crate (`crates/cerbrix-server`), with one of them also in the wall UI (`ui/src/wall`). No new subsystem: each task extends an existing module, adds its settings to the existing configuration sections, and updates the documentation it makes untrue.

**Tech Stack:** Rust (edition 2024, axum 0.8, sqlx/PostgreSQL, lettre 0.11 with rustls, figment), TypeScript/Preact for the wall, Docker Compose.

**Spec:** `docs/superpowers/specs/2026-11-14-cerbrix-design.md` (the design) and the seven GitHub issues, whose text is quoted in each task. Where an issue offers options, the decision is recorded below as D1–D9.

## Decisions

- **D1 (#88):** The shared pool gets an acquire timeout of 5 s (today: sqlx's default of 30 s). It applies to ingestion, alerting and sync too: a caller that waited 5 s for a connection gains nothing from 25 more, and all of them retry on their own.
- **D2 (#88):** A database that cannot be reached is `503` with code `database_unavailable`, in the UI API and in its access guard. The UI API gets its own time limit of 25 s, answered as `503` with code `timeout` in the JSON error format with the usual headers. The outer 30 s `408` stays as the last resort for the other routes. The agent API's answers do not change.
- **D3 (#90):** A password change whose verified hash is no longer the stored one answers `409 conflict`; the stored password and the sessions stay as the other change left them.
- **D4 (#89):** Failed logins are counted per client address in memory: 10 failures within 5 minutes refuse further logins from that address with the existing `429 too_many_attempts` until the window ends, before any password work and before the account's own counter. An IPv6 address counts as its /64. This limits password guessing; it slows down but does not prevent keeping one account locked from one address (the account lock itself is unchanged), and the documentation says so.
- **D5 (#89):** The client address is the peer address of the connection. Behind a reverse proxy the setting `auth.client_address_header` (off by default) names a header such as `X-Forwarded-For`; the right-most address in it is used, and the peer address when the header is missing or unreadable. Without the setting, everybody behind the proxy shares one counter; the documentation says so in "Behind a reverse proxy".
- **D6 (#99):** The display state names the most urgent problem behind the alarm (`worst`: host, summary, severity, start) and the number of causes is already there. The band always shows it on a second line, whether or not the dashboard has a problems panel: one behaviour, and the band is what a person reads first.
- **D7 (#42):** Metric values: the retention of VictoriaMetrics becomes a Compose variable (`CERBRIX_METRICS_RETENTION`, default `12` = months, as today). Problems: `retention.closed_problems_days`, default 365, `0` keeps them for ever; an hourly job deletes problems resolved longer ago (their notification records go with them). Open problems are never deleted. Sync history and unknown agents are not part of this (#52 covers the latter).
- **D8 (#69):** Explicit settings `gitops.proxy` and `gitops.ca_file`, passed to git as `http.proxy` and `http.sslCAInfo`. The environment stays cleared. They apply to HTTPS repositories only.
- **D9 (#80):** `smtp.ca_file` adds certificate authorities to the public ones. The EHLO name is the domain of `smtp.from` by default and `smtp.hello_name` overrides it.

## Global Constraints

- English for code, comments, messages, documentation and commit messages.
- `unsafe` is forbidden. No new runtime dependency. A dev-dependency is allowed only where a task names it.
- Every new setting: field with default and validation in `crates/cerbrix-server/src/config.rs` (a `fn validate` message in the style `"auth.session_idle_secs must be between 300 and 604800"`), a config test with `figment::Jail`, a commented line in `deploy/server.env.example`, and a row in the settings table of the document that covers the section.
- Every change to an API answer is documented in `docs/api.md`; the test `every_route_is_documented` keeps passing.
- Secrets never appear in `Debug` output, logs or error texts.
- Database tests use `#[sqlx::test(migrations = "./migrations")]`. No test sleeps longer than 1 s.
- Before each commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p cerbrix-server`; for UI changes `npm run check` in `ui/`. Do not run `npm run build` while a cargo command runs.
- Commit with explicit paths: `git commit -m "…" -- <paths>`. One commit per task unless a step says otherwise.
- Each task's last step moves the "known limitation" text it resolves out of the documentation.

## Review Focus

1. **A database that is slow, not down** (Task 1): a query that hangs after a connection was acquired must end as the JSON `503 timeout` at 25 s, not as the bare `408`.
2. **A proxy that sends several or forged `X-Forwarded-For` values** (Task 3): `X-Forwarded-For: 1.2.3.4, 10.0.0.7` counts as `10.0.0.7`; a client-supplied left part never chooses the counter.
3. **A full throttle table** (Task 3): 10 000 distinct addresses must not exhaust memory and must not free an address that is currently refused.
4. **A very long host name or summary in the alarm band** (Task 4): the text is cut with an ellipsis and the "+N more" count stays visible.
5. **A retention job on a table with a million old problems** (Task 5): deleted in batches, so no single statement holds the table for long, and the loop still stops on shutdown.

---

### Task 1: The API answers quickly and in its error format when PostgreSQL is unreachable (#88)

**Files:**
- Modify: `crates/cerbrix-server/src/state/mod.rs` (pool options, `StateError`)
- Modify: `crates/cerbrix-server/src/api/ui/mod.rs` (shared `store_failed`, time limit layer, test fixture)
- Modify: `crates/cerbrix-server/src/api/ui/{session,users,config,hosts,dashboards,displays,problems}.rs` (remove their `store_failed` copies)
- Modify: `crates/cerbrix-server/src/auth/mod.rs` (`AuthError::Unavailable`)
- Modify: `ui/src/components/describeError.ts` and its test
- Modify: `docs/api.md` ("Errors" table, a paragraph "**Database down.**" next to "**Metrics store down.**")

**Interfaces:**
- Produces: `pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);` in `state/mod.rs`; `impl StateError { pub fn is_unavailable(&self) -> bool }`; `pub(crate) fn store_failed(what: &str, e: &StateError) -> ApiError` in `api/ui/mod.rs`; `pub(crate) const UI_REQUEST_TIMEOUT: Duration = Duration::from_secs(25);`.

- [ ] **Step 1: Failing tests for `is_unavailable`** in `state/mod.rs`: `true` for `sqlx::Error::PoolTimedOut`, `PoolClosed` and `Io(_)`; `false` for `RowNotFound`, for `StateError::NotFound` and `Conflict`. A database error whose SQLSTATE starts with `08` or `57P` (connection failure, server shutting down or starting) is also unavailable; test the code check through a small private `fn sqlstate_unavailable(code: &str) -> bool` (`"08006"`, `"57P03"` true; `"23505"` false).
- [ ] **Step 2: Implement** `is_unavailable`, and set `.acquire_timeout(ACQUIRE_TIMEOUT)` in `StateStore::connect`.
- [ ] **Step 3: Failing API tests** in `api/ui/mod.rs` (plain `#[tokio::test]`, no database): build the fixture on `PgPoolOptions::new().acquire_timeout(Duration::from_millis(200)).connect_lazy("postgres://cerbrix@127.0.0.1:1/cerbrix")`.
  - `database_down_is_503_on_a_guarded_route`: `GET /api/v1/status` with any session cookie → status 503, `error_code() == "database_unavailable"`, header `cache-control: no-store`.
  - `database_down_is_503_on_login`: `POST /api/v1/session` with a name and password → 503 `database_unavailable`.
  - `slow_request_is_503_timeout`: a test-only route added to the UI router under the same layers that sleeps past a time limit of 50 ms (make the limit a parameter of the layer function so the test can pass 50 ms) → 503, code `timeout`, `cache-control: no-store`, JSON body.
- [ ] **Step 4: Implement.** One `store_failed` in `api/ui/mod.rs`: logs as the copies do; unavailable → `ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable", "the database is not reachable")`, otherwise `ApiError::internal()`. Delete the seven copies and import the shared one (`users.rs::change_failed` keeps its 404/409 mapping and falls back to it). `AuthError::Unavailable` for a guard whose store error `is_unavailable()`, answered the same way. The time limit is an axum `from_fn` middleware inside `response_headers` (so its answer carries the headers) using `tokio::time::timeout`, answering `ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "timeout", "the request took too long")`.
- [ ] **Step 5: UI.** `describeError` says "The server cannot reach its database." for `database_unavailable` and "The server took too long to answer." for `timeout`; test both. Confirm with a test in `ui/src/session.test.tsx` that a `503` on `/session` leads to the "unreachable" gate with its automatic retry, not to the login form.
- [ ] **Step 6: Docs.** Add `database_unavailable` 503 and `timeout` 503 to the code table and the "**Database down.**" paragraph (answers within about 5 s; the UI retries by itself).
- [ ] **Step 7: Run** the commands of Global Constraints; commit `fix(server): a database that is down answers 503 within seconds (#88)`.

### Task 2: A password change cannot overwrite a concurrent reset (#90)

**Files:**
- Modify: `crates/cerbrix-server/src/state/users.rs` (`PasswordCheck`, `check_password`, `store_password`, `set_user_password_renewing_session`)
- Modify: `crates/cerbrix-server/src/api/ui/session.rs` (`change_password`)
- Modify: `docs/api.md` (`PUT /api/v1/session/password`)

**Interfaces:**
- Produces: `PasswordCheck::Right { hash: String }` (the enum loses `Copy`); `set_user_password_renewing_session(&self, id: Uuid, verified_hash: &str, password_hash: &str, now: OffsetDateTime, cfg: &AuthConfig) -> Result<String, StateError>`; private `store_password(&self, id, expected: Option<&str>, password_hash, now)`.

- [ ] **Step 1: Failing test** `password_change_racing_an_admin_reset_changes_nothing` in `state/users.rs`, modelled on `login_racing_a_password_change_gets_no_session`: create a user, `check_password` → `Right { hash }`; `set_user_password(id, other_hash, now)` (the admin reset); then `set_user_password_renewing_session(id, &hash, new_hash, now, cfg)` → `Err(StateError::Conflict(_))`; the stored hash is `other_hash`; the user has no session.
- [ ] **Step 2: Implement.** `store_password` adds `AND password_hash = $n` when `expected` is `Some`; with zero rows it tells an unknown user (`NotFound`) from a changed hash (`Conflict("the password was changed in the meantime")`) by a second read in the same transaction. `set_user_password` passes `None`.
- [ ] **Step 3: Handler.** `change_password` passes the verified hash; `Conflict` → `ApiError::new(StatusCode::CONFLICT, "conflict", "the password was changed in the meantime")`. Test `change_password_conflict_is_409` at the handler level is not possible without the race; instead assert the mapping in a unit test of a small private `fn change_error(e: &StateError) -> ApiError`.
- [ ] **Step 4: Docs** — add the `409` to the route's description. Run the commands; commit `fix(server): a password change no longer overwrites a concurrent reset (#90)`.

### Task 3: Failed logins are throttled per client address (#89)

**Files:**
- Create: `crates/cerbrix-server/src/auth/throttle.rs`
- Modify: `crates/cerbrix-server/src/auth/mod.rs` (`pub mod throttle;`)
- Modify: `crates/cerbrix-server/src/config.rs` (`AuthConfig.client_address_header`)
- Modify: `crates/cerbrix-server/src/api/mod.rs` (`AppState.logins`), `src/tls.rs` (connect info, state), `src/api/ui/mod.rs` (fixture), `src/api/ui/session.rs` (`login`)
- Modify: `docs/install.md` ("Login settings", "Behind a reverse proxy"), `docs/api.md` ("**Failed logins.**"), `deploy/server.env.example`

**Interfaces:**
- Produces, in `auth/throttle.rs`:
  - `pub const MAX_FAILURES: u32 = 10;` `pub const WINDOW: Duration = Duration::from_secs(300);` `const MAX_ENTRIES: usize = 10_000;`
  - `#[derive(Default)] pub struct LoginThrottle` (a `Mutex<HashMap<IpKey, Entry>>`), with `pub fn refused(&self, addr: IpAddr, now: Instant) -> bool` and `pub fn failed(&self, addr: IpAddr, now: Instant)`.
  - `pub fn client_address(peer: IpAddr, headers: &HeaderMap, header: Option<&HeaderName>) -> IpAddr`.
- `AppState.logins: Arc<LoginThrottle>`; `AuthConfig.client_address_header: Option<String>` (env `CERBRIX_AUTH__CLIENT_ADDRESS_HEADER`, default none; validation: a valid HTTP header name, message `"auth.client_address_header must be a header name such as X-Forwarded-For"`).

- [ ] **Step 1: Failing unit tests** in `throttle.rs`:
  - `ten_failures_refuse_the_address_until_the_window_ends`: nine `failed` → `refused` false; the tenth → true; at `first + WINDOW` → false, and the count starts again.
  - `another_address_is_not_affected`.
  - `ipv6_counts_per_64`: failures from `2001:db8:1:2::1` refuse `2001:db8:1:2:ffff::9`, not `2001:db8:1:3::1`.
  - `a_full_table_drops_only_ended_windows`: fill `MAX_ENTRIES` addresses of which one is refused and the rest have ended windows; a new address is recorded and the refused one stays refused. When no window has ended, the new address is not recorded (and not refused) rather than clearing the table.
  - `client_address`: no header setting → peer; `X-Forwarded-For: 1.2.3.4, 10.0.0.7` → `10.0.0.7`; header missing, empty or `garbage` → peer; several header lines → the last line's right-most value.
- [ ] **Step 2: Implement** `throttle.rs`. A window starts with an address's first failure and ends `WINDOW` later; the first refusal of a window logs one `tracing::warn!(%address, "logins from this address are refused for up to 5 minutes after repeated failures")`.
- [ ] **Step 3: Failing handler tests** in `api/ui/session.rs` (the fixture's `call` gains a way to set the peer address; default `127.0.0.1`):
  - `eleventh_failed_login_from_an_address_is_429`: ten wrong logins for an unknown name → each 401; the eleventh → 429 `too_many_attempts`; the same request from another peer address → 401.
  - `a_refused_address_does_not_count_against_the_account`: a real user, four wrong passwords from address A (account counter 4), then A is filled up to the limit with an unknown name; further wrong passwords for the real user from A answer 429 and the user can still log in from address B with the right password.
  - `a_successful_login_is_not_counted`, and `a_busy_server_is_not_counted` (use `use_password_gate` with an occupied gate).
  - `the_configured_header_chooses_the_address`: with `client_address_header = "x-forwarded-for"`, failures with `X-Forwarded-For: 203.0.113.5` refuse that address only.
- [ ] **Step 4: Implement.** `tls.rs` serves with `into_make_service_with_connect_info::<SocketAddr>()` (both branches); `login` takes `ConnectInfo<SocketAddr>` and `HeaderMap`, answers `passwords_busy()` when `refused`, and calls `failed` for `Wrong` and `Locked`. The answer still does not depend on whether the name exists.
- [ ] **Step 5: Docs.** "Login settings": the new row and a paragraph with D4's numbers and its honest limit. "Behind a reverse proxy": set `CERBRIX_AUTH__CLIENT_ADDRESS_HEADER=X-Forwarded-For` only when every request passes the proxy and the proxy appends to or sets that header; without it all users share one counter. `docs/api.md`: the `429` of the login now also means a refused address.
- [ ] **Step 6: Run** the commands; commit `feat(server): failed logins are throttled per client address (#89)`.

### Task 4: The alarm band names the problem (#99)

**Files:**
- Modify: `crates/cerbrix-server/src/view/display.rs` (`DisplayTarget`, `alarm_target`)
- Modify: `crates/cerbrix-server/src/api/ui/displays.rs` (shape tests only)
- Modify: `docs/api.md` (`GET /api/v1/displays`, `GET /api/v1/display/{id}`: field table and examples)
- Modify: `ui/src/api/types.ts`, `ui/src/wall/{WallFrame.tsx,DisplayPage.tsx,wall.css}` and their tests; `ui/e2e/smoke.spec.ts`

**Interfaces:**
- Produces (server): `#[derive(Serialize, PartialEq, Debug)] pub struct WorstProblem { pub host: String, pub summary: String, pub severity: Severity, #[serde(with = "time::serde::rfc3339")] pub started_at: OffsetDateTime }`; `DisplayTarget.worst: Option<WorstProblem>` — the problem of `causes[0]`, `None` exactly when `target` is `None`. JSON: `"worst": {"host":"web-01","summary":"…","severity":"critical","started_at":"…"}` or `null`.
- Produces (UI): `interface WorstProblem { host: string; summary: string; severity: Severity; started_at: string }`; `worst: WorstProblem | null` on `DisplayState` and `Display`; `WallAlarm` gains `problem?: { severity: Severity; host: string; summary: string; more: number }` (`more` = `causes.length - 1`).

- [ ] **Step 1: Failing server tests** in `view/display.rs`: `alarm_target_names_the_most_urgent_problem` (two alarms on the target: `worst` is the more severe one, and with equal severity the older one, with its host name and summary); `no_alarm_has_no_worst`; update `DisplayTarget::none()` and the key lists in `displays_list_shape_with_target_and_queue` and `display_state_with_token`.
- [ ] **Step 2: Implement:** keep the `&Problem` of each alarm through the sort in `alarm_target` and build `worst` from the first.
- [ ] **Step 3: Failing UI tests.** `WallFrame.test.tsx`: with `problem: {severity: 'critical', host: 'web-01', summary: 'Disk almost full', more: 0}` the band has a second line with the exact text `Critical: web-01 – Disk almost full`; with `more: 2` the line ends with a separate element reading `+2 more`; without `problem` the band has one line as today. `DisplayPage.test.tsx`: a state with `worst` shows the line; the existing exact-text assertions select the first line. `wall.test.ts` keeps passing (the new line uses `wall-band-line wall-band-detail`, its text `wall-band-text`, its count `wall-band-more`).
- [ ] **Step 4: Implement** the second line in `AlarmBand` (severity word from the UI's existing severity label helper) and `bandOf` in `DisplayPage.tsx`.
- [ ] **Step 5: Smoke run and docs.** `ui/e2e/smoke.spec.ts`: the alarm band of the display contains the host name of the end-to-end host. `docs/api.md`: the `worst` row and the examples; `docs/web-ui.md` "Wall displays": the band names the most urgent problem.
- [ ] **Step 6: Run** the commands (server and `npm run check`); commit `feat: the alarm band names the most urgent problem (#99)`.

### Task 5: Retention for metric values and closed problems (#42)

**Files:**
- Create: `crates/cerbrix-server/src/retention.rs`
- Modify: `crates/cerbrix-server/src/lib.rs`, `src/tls.rs` (spawn the loop), `src/config.rs` (`RetentionConfig`), `src/state/alerting.rs` (the delete)
- Modify: `docker-compose.yml`, `deploy/server.env.example`, `docs/install.md` (new section "Retention"), `docs/alerting.md` (one sentence in the problems section)

**Interfaces:**
- Produces: `#[serde(default)] pub struct RetentionConfig { pub closed_problems_days: u32 }` as `Config.retention` (default 365; valid: 0 or 1..=3650; message `"retention.closed_problems_days must be 0 (keep for ever) or between 1 and 3650"`; env `CERBRIX_RETENTION__CLOSED_PROBLEMS_DAYS`).
- `pub async fn delete_problems_resolved_before(&self, before: OffsetDateTime, limit: i64) -> Result<u64, StateError>` on `StateStore` (deletes at most `limit` problems with `resolved_at < before`, oldest first).
- In `retention.rs`: `pub const RETENTION_EVERY: Duration = Duration::from_secs(3600);` `const BATCH: i64 = 5_000;` `pub async fn run_retention_loop(store: StateStore, cfg: RetentionConfig, every: Duration, shutdown: CancellationToken)`.

- [ ] **Step 1: Failing state tests** in `state/alerting.rs`: `old_closed_problems_are_deleted_with_their_notifications` (a problem resolved 400 days ago with an outbox row, one resolved 10 days ago, one open since 500 days; `before = now − 365 d` deletes exactly the first and its outbox row); `the_limit_bounds_one_call` (three old problems, `limit = 2` → returns 2, the oldest two are gone).
- [ ] **Step 2: Implement** the delete as `DELETE FROM problems WHERE id IN (SELECT id FROM problems WHERE resolved_at < $1 ORDER BY resolved_at LIMIT $2)`. If no index serves it, add migration `0006_retention.sql` with `CREATE INDEX problems_resolved ON problems (resolved_at) WHERE resolved_at IS NOT NULL;`.
- [ ] **Step 3: Failing loop tests** in `retention.rs`, modelled on `expired_sessions_are_deleted_by_the_loop`: `the_loop_deletes_expired_problems`; `zero_days_deletes_nothing`; `the_loop_stops_on_shutdown`.
- [ ] **Step 4: Implement** the loop after the pattern of `run_session_cleanup_loop`: each tick repeats batches until one returns fewer than `BATCH` or shutdown is requested, logs the total at `info` when it is not zero and errors at `error`. With `closed_problems_days == 0` the loop is not spawned. Spawn it in `tls::serve` next to the session cleanup.
- [ ] **Step 5: Config tests** (`retention_defaults_and_bounds`) and implementation.
- [ ] **Step 6: Compose and docs.** `docker-compose.yml`: `-retentionPeriod=${CERBRIX_METRICS_RETENTION:-12}`. `docs/install.md` "Retention": both settings, what each removes, that a number without a unit means months for VictoriaMetrics and `90d`/`2y` are possible, that shortening it deletes values, that open problems are never deleted, and that sync history is kept. Update the line "`victoriametrics` (12 months retention)".
- [ ] **Step 7: Run** the commands and `docker compose config >/dev/null`; commit `feat(server): retention for closed problems, configurable retention for metric values (#42)`.

### Task 6: Git fetches through a proxy and with a private CA (#69)

**Files:**
- Modify: `crates/cerbrix-server/src/config.rs` (`GitopsConfig`), `src/gitops/git.rs` (`git_config`, secrets, tests)
- Modify: `deploy/server.env.example`, `docs/install.md` ("Configuration from Git": settings table, a subsection "Behind a proxy or with a private CA"; remove the limitation in "Limits to know")

**Interfaces:**
- Produces: `GitopsConfig.proxy: Option<String>` (env `CERBRIX_GITOPS__PROXY`), `GitopsConfig.ca_file: Option<PathBuf>` (env `CERBRIX_GITOPS__CA_FILE`). `Debug` shows the proxy with its user info masked.

- [ ] **Step 1: Failing config tests:** `proxy` accepts `http://`, `https://`, `socks5://` and `socks5h://` URLs and rejects anything else, whitespace and control characters (`"gitops.proxy must be an http, https or socks5 URL"`); `proxy` or `ca_file` with a repository URL that is not HTTPS is invalid (`"gitops.proxy and gitops.ca_file need an https repository URL"`); `Debug` of a config with `http://user:secret@proxy:3128` does not contain `secret`.
- [ ] **Step 2: Failing git tests** in `gitops/git.rs`: `git_config` contains `http.proxy` and `http.sslCAInfo` with the configured values and neither without them; `git_environment_is_minimal` still passes; `fetch_goes_through_the_proxy_and_hides_its_password`: a local TCP listener stands in for the proxy, the repository URL is `https://example.invalid/repo.git`, the proxy `http://user:secret@127.0.0.1:<port>`; the listener reads a first line starting with `CONNECT example.invalid:443` and closes; the fetch fails and its error text does not contain `secret`.
- [ ] **Step 3: Implement.** Add the two entries in `git_config`; add the proxy's password (when the URL has one) to the runner's secrets next to the token.
- [ ] **Step 4: Docs** as listed, including that SSH repositories are not affected by these settings and that the CA file replaces git's default bundle for the fetch (so it must contain the authority that signed the Git server's certificate). Run the commands; commit `feat(server): gitops.proxy and gitops.ca_file (#69)`.

### Task 7: SMTP trusts an internal CA and sends a proper EHLO name (#80)

**Files:**
- Modify: `crates/cerbrix-server/src/config.rs` (`SmtpConfig`), `src/alerting/channel.rs` (`SmtpChannel::new`, tests), `crates/cerbrix-server/Cargo.toml` (dev-dependency `tokio-rustls` only if the TLS test server needs it and it is not already a dependency)
- Modify: `deploy/server.env.example`, `docs/alerting.md` (SMTP settings table; remove both limitation texts), `docs/install.md` (table; remove the limitation sentence)

**Interfaces:**
- Produces: `SmtpConfig.ca_file: Option<PathBuf>` (env `CERBRIX_SMTP__CA_FILE`), `SmtpConfig.hello_name: Option<String>` (env `CERBRIX_SMTP__HELLO_NAME`); `pub(crate) fn hello_name(cfg: &SmtpConfig) -> Option<String>` in `channel.rs` (the setting, else the domain of `from`, else `None` = lettre's default).

- [ ] **Step 1: Failing config tests:** `hello_name` must be a host name (letters, digits, `-`, `.`, 1–253 characters; `"smtp.hello_name must be a host name"`); `ca_file` with `encryption = "none"` is invalid (`"smtp.ca_file needs starttls or tls"`).
- [ ] **Step 2: Failing channel tests:**
  - `ehlo_uses_the_domain_of_from`: with `from = "Cerbrix <alerts@example.net>"` the captured command is `EHLO example.net`; `ehlo_uses_the_configured_name` with `hello_name = "mon.example.net"`.
  - `a_relay_with_an_internal_ca_is_trusted_with_ca_file`: `rcgen` makes a CA and a server certificate for `localhost`; a fake server speaks implicit TLS (`encryption = "tls"`, host `localhost`) and then the existing script; with `ca_file` pointing at the CA's PEM the message is delivered.
  - `a_relay_with_an_internal_ca_is_refused_without_ca_file`: the same server without `ca_file` → sending fails.
  - `an_unreadable_ca_file_fails_at_start`: `SmtpChannel::new` returns an error naming the file (and nothing of its content).
- [ ] **Step 3: Implement** with lettre's `TlsParameters::builder(host)` and `add_root_certificate(Certificate::from_pem(..))` for each certificate in the file (check lettre 0.11's documentation for the exact calls and that the public roots stay trusted), and `.hello_name(ClientId::Domain(..))`.
- [ ] **Step 4: Docs** as listed. Run the commands; commit `feat(server): smtp.ca_file and smtp.hello_name (#80)`.

---

## After the last task

- `scripts/e2e.sh` passes (it exercises login, the wall display with its alarm band, e-mail and Git).
- `cargo test --workspace` on Windows and in the Linux container.
