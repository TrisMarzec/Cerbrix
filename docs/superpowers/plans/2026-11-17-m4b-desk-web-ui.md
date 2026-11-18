# M4b: Desk Web UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The server serves a web UI at `/` with which a person at a desk logs in, sees dashboards, hosts with graphs, problems (acknowledge, close), the configuration state, and manages keys, display tokens and users.

**Architecture:** A single-page application in `ui/` (TypeScript, Preact, Vite) that talks only to `/api/v1` from M4a and polls. `npm run build` writes `ui/dist`; a build script of `cerbrix-server` embeds those files into the binary (or a placeholder page when `ui/dist` does not exist, so `cargo build` never needs Node). The server serves hashed assets as immutable, `index.html` for every other non-API path, with a strict content security policy. M4 stays three plans: M4a (API, merged), this one, and M4c (rendering a dashboard on the panel grid and the wall display mode).

**Tech Stack:** TypeScript (strict), Preact 10, Vite, Vitest with Testing Library and jsdom, uPlot for graphs, `@fontsource/barlow-semi-condensed`; Rust (axum) for serving; Playwright (Chromium, in its Docker image) for one smoke run in `scripts/e2e.sh`.

**Spec:** `docs/superpowers/specs/2026-11-15-ui-design-concept.md` (sections 7 and 8.2–8.7 for this plan), on top of `docs/superpowers/specs/2026-11-14-cerbrix-design.md`. The API is `docs/api.md`. The intended look is `docs/ui/mockups/desk-views.html` (open it in a browser; its CSS is the reference for spacing and density).

## Decisions

| # | Decision | Why |
|---|---|---|
| C1 | Preact 10 with hooks, no state library, a router of our own (about 80 lines) | The React model is the most widely known; the app is seven pages and polls, so a store and a router package would be weight without use |
| C2 | Graphs with uPlot | Small, fast, made for time series; draws on a canvas, so it works under a strict content security policy |
| C3 | The font comes from the npm package `@fontsource/barlow-semi-condensed` (weights 400, 500, 600, 700, `latin` and `latin-ext`) and is bundled | The concept forbids a request to a font service |
| C4 | `ui/dist` is not committed. `crates/cerbrix-server/build.rs` embeds it, or a placeholder page saying how to build the UI | `cargo build` and `cargo test` must work without Node; a release must not ship without the UI (the Dockerfile and CI build it first, and a test in CI fails on the placeholder) |
| C5 | Polling every 10 seconds on the desk, paused while the tab is hidden, at once when it becomes visible | The API has no push channel; 10 s is enough at a desk (the wall's 5 s belongs to M4c) |
| C6 | Dark theme only | The approved concept lists a light theme as a non-goal; issue #31's "light and dark theme" criterion is amended |
| C7 | The Dashboards page lists dashboards and wall displays; a dashboard row opens the Problems view filtered to that dashboard. "Open on this screen" and the grid come with M4c | Rendering panels on the grid is the wall's core and is built once, there |
| C8 | Filters, tabs, search text and the page number live in the URL query | A view can be bookmarked and sent to a colleague; reload keeps it |
| C9 | Times are shown in the browser's time zone, 24-hour, as `14:05` today and `16 Nov 14:05` otherwise, with the full RFC 3339 time as tooltip; durations as `2 h 14 min` | Operators think in local time; the tooltip settles disputes |
| C10 | Secrets shown once (enrolment key, display token, generated password) appear in a dialog with a copy button and are kept only in component state | They must not survive in storage, the URL or the history |
| C11 | Operators can also close a problem (required comment) from the problem's row menu | The API has it, and the concept's anti-goal is an alarm nobody can clear from a desk |
| C12 | The open-problem order of the API changes to: unacknowledged first, most severe first, then **newest** first | The concept (5.1) says so; the alarm target's tie-break (older alarm wins, section 6) is a different rule and stays |
| C13 | The API gains, additively: `agent_version` and `dashboards` on the host detail, and `dashboards` (host name → dashboards) on the problems list | Concept 8.5 and 8.6 show them (issue #86) |
| C14 | The agent version is taken from the `User-Agent` header (`cerbrix-agent/<version>`) the agent already sends with every request, and stored when values arrive | It stays right after an agent upgrade, without a protocol change |
| C15 | One Playwright smoke run (Chromium) at the end of `scripts/e2e.sh`, in the Playwright Docker image on the stack's network; `CERBRIX_E2E_SKIP_UI=1` skips it | Embedding, the security headers, the cookie and the fallback route only fail in a real browser; the host needs no browser installed |
| C16 | Licence texts of everything bundled (Preact, uPlot: MIT; the font: OFL) are collected into `licenses.txt`, served at `/licenses.txt` and linked from the login page | MIT and OFL require the notice to travel with the distribution |

## Global Constraints

- English everywhere: UI text, code, comments, commit messages, docs. Labels are sentence case.
- Apache-2.0 project: npm dependencies that end up in the bundle must be MIT, ISC, BSD or OFL. Development-only dependencies may also be Apache-2.0.
- The served UI makes no request to any other host. Content security policy: `default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'`.
- Text from the API (host names, trigger names, summaries, comments, user names, file names, reasons) is rendered as text. `dangerouslySetInnerHTML`, `innerHTML` and `eval` are forbidden; a test greps the source for them.
- Colours come only from the tokens of concept section 7 (CSS custom properties in `ui/src/styles/tokens.css`). Severity is never shown by colour alone: the word is there too. Severity `info` uses Dim.
- Numbers are tabular (`font-variant-numeric: tabular-nums`) everywhere.
- Below 860 px the navigation moves to the top (concept 8.2). Everything is reachable by keyboard; the focus outline is 2 px in the Git colour; `prefers-reduced-motion` turns transitions off.
- TypeScript `strict`, no `any`, no `@ts-ignore`. `npm run check` (type check, tests) has no warnings and no skipped tests.
- Rust: `unsafe` is forbidden; `cargo fmt`, `cargo clippy -- -D warnings` and all tests stay green; `cargo build --workspace --locked` works on a checkout without `ui/dist` and without Node.
- The UI never decides access by itself: buttons are hidden by role for convenience, and every `403` from the server is shown as a message.
- Node 22 or newer; `package-lock.json` is committed; CI uses `npm ci`.
- JavaScript of the built UI is at most 150 kB gzip, CSS at most 30 kB gzip, fonts excluded; `npm run build` fails above that.

## Review Focus

1. **Hostile or odd text from the API** — a host name, comment or sync reason containing `<script>`, `&`, quotes, or 200 characters without a space: shown literally, the layout does not break (long text wraps or is cut with an ellipsis and a tooltip). Test in Task 3 (`ProblemRow`) and Task 9 (sync issue).
2. **The session ends while a page is open** (idle limit, server restart, password change elsewhere) — the next poll gets `401`: the login page appears with "Your session has ended", and after login the user is back on the same URL with the same filters. A lost network connection instead keeps the last data, marks it stale and says since when. Test in Task 4.
3. **Role limits** — a viewer sees no Acknowledge, Close or admin controls; opening `/administration` as an operator shows "Not allowed for your role", not a broken page; a `403` or `409` answer to an action is shown in the dialog that caused it and the dialog stays open. Tests in Tasks 4, 8 and 10.
4. **Empty and partial data** — no hosts, no dashboards, a configuration that never synced, `metrics_available: false`, a graph without points, a host with thirty disks, a list total that shrinks below the current page: each has a plain sentence instead of an empty box or an endless spinner. Tests in Tasks 5–9.
5. **Deep links and upgrades** — reloading `/hosts/db-02` or `/problems?state=resolved&page=3` works (the server answers `index.html`); names that need encoding (`C:\` as a mount, a host name with dots) survive the round trip through the URL; after a server upgrade the browser does not run the old script (`index.html` is `no-cache`, assets are content-hashed). Tests in Tasks 2, 4 and 7.

## File Structure

```
ui/
  package.json, package-lock.json, tsconfig.json, vite.config.ts, index.html
  scripts/check-size.mjs        fails the build above the size budget
  scripts/licenses.mjs          writes dist/licenses.txt from the bundled packages
  src/main.tsx                  mounts <App/>
  src/app.tsx                   session gate, routes → pages
  src/styles/tokens.css         colours, type scale, spacing (the only place with colour values)
  src/styles/base.css           reset, tables, forms, buttons, layout
  src/api/client.ts             fetch wrapper, ApiError
  src/api/types.ts              the JSON shapes of docs/api.md
  src/lib/router.ts             path + query routing, <Link>
  src/lib/poll.ts               usePolled
  src/lib/format.ts             times, durations, numbers, severity words
  src/lib/query.ts              reading and writing the URL query for filters and paging
  src/session.tsx               SessionProvider, useSession
  src/components/               ProblemRow, HostSquares, StateText, Dialog, SecretDialog,
                                Pager, Empty, Banner, Graph, TimeText
  src/shell/                    Shell (navigation, top bar), LoginPage, PasswordDialog
  src/pages/                    DashboardsPage, HostsPage, HostPage, ProblemsPage,
                                ConfigurationPage, AdministrationPage, NotFoundPage
  src/test/                     setup.ts, fixtures.ts (sample API answers), render.tsx
  e2e/smoke.spec.ts             the Playwright smoke run
crates/cerbrix-server/
  build.rs                      embeds ui/dist or the placeholder
  src/webui.rs                  serves the embedded files with caching and security headers
  migrations/0005_agent_version.sql
```

Every page file owns its data loading and its URL state; components take data through props and never call the API. Tests sit next to the file they test (`x.test.tsx`).

---

### Task 1: API additions for the desk (issue #86, order of open problems)

**Files:**
- Create: `crates/cerbrix-server/migrations/0005_agent_version.sql`
- Modify: `crates/cerbrix-server/src/api/agent.rs`, `src/state/alerting.rs` (where `last_seen_at` is written), `src/state/read.rs`, `src/view/mod.rs`, `src/view/dashboard.rs`, `src/api/ui/hosts.rs`, `src/api/ui/problems.rs`, `docs/api.md`

**Interfaces:**
- Produces (JSON, additive):
  - `GET /api/v1/hosts/{name}`: `"agent_version": "0.1.0" | null`, `"dashboards": [{"id": "kunde-a", "name": "Kunde A"}]` (sorted by name).
  - `GET /api/v1/problems`: top-level `"dashboards": {"<host name>": [{"id", "name"}]}` with one entry for every host that appears in `items` (an empty list for a host in no dashboard).
  - Open problems (list and `problems` panel): unacknowledged first, most severe first, newest `started_at` first, then id.

- [ ] **Step 1: Write the failing tests**
  - Store: `agent_version_is_recorded_from_values` (version `1.2.3` given → stored; a later `1.3.0` replaces it; `None` leaves it); `agent_version_is_not_rewritten_when_equal` (the row's `xmin` or an updated-rows count shows no write).
  - Handler: `values_take_the_version_from_the_user_agent` (`User-Agent: cerbrix-agent/1.2.3` → stored `1.2.3`; a header that is missing, longer than 64 characters, or not of that form → `null`, request still accepted).
  - `host_detail_has_agent_version_and_dashboards` (host in two dashboards, sorted by name; host in none → `[]`).
  - `problems_list_names_dashboards_of_its_hosts` (two hosts on the page, one in two dashboards; a host on another page is absent from the map).
  - `open_problems_are_newest_first_within_a_severity` for the list and for the dashboard's problems panel; `alarm_target_still_prefers_the_older_alarm` (existing test must stay green; add it if absent).
  - `problems_list_with_many_hosts`: 300 hosts in 3 dashboards, a page of 50 problems → the map has exactly the hosts of the page. The handler may add at most two store calls (dashboards, active hosts) to what it does now, and none per host or per problem.
- [ ] **Step 2: Run** `cargo test -p cerbrix-server agent_version dashboards_of newest_first` — expected: fail to compile or fail.
- [ ] **Step 3: Implement.** Migration: `ALTER TABLE hosts ADD COLUMN agent_version text;`. Parse the version with a small pure function `agent_version_from_user_agent(&str) -> Option<&str>` (prefix `cerbrix-agent/`, then 1–64 characters of `[0-9A-Za-z.+-]`). Write it in the statement that already updates `last_seen_at`, only when it differs. The list order and the alarm urgency must be two functions; do not change `urgency()`.
- [ ] **Step 4: Update `docs/api.md`** (host detail, problems list, the order sentence) and run `cargo test -p cerbrix-server` — expected: all pass, `every_route_is_documented` included.
- [ ] **Step 5: Commit** `feat(server): agent version, dashboards of a host, newest-first open problems`

---

### Task 2: UI project, embedding and serving

**Files:**
- Create: `ui/package.json`, `ui/tsconfig.json`, `ui/vite.config.ts`, `ui/index.html`, `ui/src/main.tsx`, `ui/src/app.tsx` (one line of text for now), `ui/src/test/setup.ts`, `ui/scripts/check-size.mjs`, `ui/scripts/licenses.mjs`, `crates/cerbrix-server/build.rs`, `crates/cerbrix-server/src/webui.rs`
- Modify: `crates/cerbrix-server/Cargo.toml` (`build = "build.rs"`, `sha2` as build dependency), `src/api/mod.rs` (mount), `src/lib.rs` or `main.rs` (module), `.gitignore` (`ui/node_modules`, `ui/dist`), `Dockerfile` (a Node stage that builds `ui/dist` before the Rust stage), `.github/workflows/ci.yml` (job `ui`: `npm ci`, `npm run check`, `npm run build`; the `e2e` job needs nothing new because the Dockerfile builds the UI), `.github/workflows/release.yml` only if it builds the server outside the Dockerfile, `CONTRIBUTING.md`

**Interfaces:**
- Produces:
  - npm scripts: `dev` (Vite with `/api` and `/healthz` proxied to `http://127.0.0.1:8443`), `build` (type check, Vite build, `licenses.mjs`, `check-size.mjs`), `check` (`tsc --noEmit` and `vitest run`), `test` (`vitest`).
  - Vite writes `dist/index.html`, `dist/assets/<name>-<hash>.<ext>`, `dist/licenses.txt`. No inline script or style in `index.html`.
  - `build.rs` generates `$OUT_DIR/webui_assets.rs`: `pub static ASSETS: &[Asset]` with `Asset { path: &'static str, bytes: &'static [u8], content_type: &'static str, etag: &'static str }` and `pub const PLACEHOLDER: bool`. It reads `ui/dist` relative to the workspace root; without `ui/dist/index.html` it embeds one placeholder `index.html` ("The web UI was not built. Run `npm ci && npm run build` in `ui/` and build the server again.") and sets `PLACEHOLDER = true`. It prints `cargo:rerun-if-changed` for `ui/dist`.
  - `webui::router() -> Router` mounted last in `api::router`: `GET`/`HEAD` only.

Serving rules:

| Request | Answer |
|---|---|
| `/assets/<file>` that exists | 200, its content type, `Cache-Control: public, max-age=31536000, immutable`, `ETag` |
| `/assets/<file>` that does not exist | 404 plain text |
| `/licenses.txt` | 200 `text/plain; charset=utf-8`, `no-cache` |
| any path starting with `/api/` or equal to `/healthz` | untouched (the API's own 404 stays JSON) |
| any other `GET`/`HEAD` path (`/`, `/hosts/db-02`, `/display/x`) | 200 `index.html`, `Cache-Control: no-cache`, `ETag`; `If-None-Match` → 304 |
| other methods | 405 |

Every answer of `webui` carries the content security policy from Global Constraints, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `Cross-Origin-Opener-Policy: same-origin`.

- [ ] **Step 1: Write the failing Rust tests** in `webui.rs` (against the router, working with the placeholder and with real assets alike — tests look assets up in `ASSETS` rather than naming files): `root_serves_index_no_cache_with_csp`; `deep_link_serves_index` (`/hosts/db-02`, `/problems?state=resolved`, `/display/x`); `api_paths_are_not_swallowed` (`/api/v1/nope` is the JSON 404, `/api/agent/v1/nope` is not `index.html`, `/healthz` still answers); `asset_is_immutable_and_typed` (for each of `ASSETS` under `assets/`); `unknown_asset_is_404_not_index`; `if_none_match_gives_304`; `post_to_a_page_is_405`; `path_traversal_is_not_served` (`/assets/../Cargo.toml`, `/assets/%2e%2e/x`); `placeholder_says_how_to_build` (only when `PLACEHOLDER`).
- [ ] **Step 2: Write the failing build test:** a step "The server embeds the real UI" in the CI job `ui` (which therefore also installs the Rust toolchain): after `npm run build`, `cargo test -p cerbrix-server webui::tests::ui_is_embedded -- --ignored`, a test that asserts `!PLACEHOLDER` and that `index.html` references at least one existing `assets/*.js`.
- [ ] **Step 3: Run** `cargo test -p cerbrix-server webui` — expected: fails to compile.
- [ ] **Step 4: Implement** the UI project, `build.rs`, `webui.rs`, the Dockerfile stage (Node 22 image, `npm ci && npm run build`, then `COPY --from` the `dist` into the Rust stage before `cargo build`), CI, `CONTRIBUTING.md` (Node, the three npm commands, `npm run dev` beside a running server).
- [ ] **Step 5: Run** `cargo test -p cerbrix-server webui` without `ui/dist` (placeholder) and again after `npm ci && npm run build` in `ui/` including the ignored test; `npm run check`; `docker build .` — expected: all pass.
- [ ] **Step 6: Commit** `feat: web UI project, embedded and served by the server`

---

### Task 3: Design tokens, base styles and shared components

**Files:**
- Create: `ui/src/styles/tokens.css`, `ui/src/styles/base.css`, `ui/src/lib/format.ts`, `ui/src/api/types.ts` (the types this task needs: `Severity`, `Problem`, `HostState`), `ui/src/components/{ProblemRow,HostSquares,StateText,Dialog,SecretDialog,Pager,Empty,Banner,TimeText}.tsx`, tests beside each, `ui/src/test/fixtures.ts`, `ui/src/test/render.tsx`

**Interfaces:**
- Produces:

```ts
// api/types.ts
export type Severity = 'info' | 'warning' | 'high' | 'critical';

// lib/format.ts
export function severityWord(s: Severity): string;            // "Critical", "High", "Warning", "Info"
export function duration(fromIso: string, to: Date): string;  // "45 s", "12 min", "2 h 14 min", "3 d 4 h"
export function clock(iso: string, now: Date): string;        // "14:05" today, "16 Nov 14:05" otherwise
export function age(iso: string | null, now: Date): string;   // "12 s ago", "never"
export function value(key: string, v: number | null): string; // "–" for null; *_percent and cpu.usage → "46.6 %"; bytes → "1.2 GiB"; else up to 3 significant decimals

// components
ProblemRow(props: { problem: Problem; now: Date; showHost?: boolean; actions?: ComponentChildren })
HostSquares(props: { hosts: { name: string; state: HostState | null }[]; max?: number })   // squares + "and 12 more"
StateText(props: { state: HostState | null })       // "No problems" | "Critical: Disk almost full" | "… (acknowledged)" | "… and 2 more"
Dialog(props: { title: string; onClose: () => void; children })   // modal, focus trapped, Escape closes, focus returns
SecretDialog(props: { title: string; secret: string; note: string; onClose: () => void })  // value in a read-only field, Copy button, "This is shown once."
Pager(props: { total: number; limit: number; offset: number; onChange: (offset: number) => void })
Empty(props: { children })                          // a plain sentence for an empty list
Banner(props: { kind: 'warning' | 'info'; children })
TimeText(props: { iso: string; now: Date })         // clock() with the full time as title
```

- Severity classes: `sev-critical`, `sev-high`, `sev-warning`, `sev-info`; acknowledged adds `is-acknowledged` (half strength, concept 5.1 host state rule). The problem row follows concept section 7: bar on the left edge, host in the severity colour, summary, second line "Severity · acknowledged by anna, 14:05 · comment", duration on the right.

- [ ] **Step 1: Write the failing tests** (Vitest + Testing Library):
  - `format.test.ts`: `duration` at 0 s, 59 s, 60 s, 3599 s, 2 h 14 min, 3 d 4 h, and a start in the future (→ `0 s`); `clock` today / other day / other year (adds the year); `age(null)` → `never`; `value` for `null`, `cpu.usage` 29.4078 → `29.4 %`, `memory.used` 1288490188.8 → `1.2 GiB`, `load.1m` 0.123456 → `0.123`.
  - `ProblemRow.test.tsx`: renders host, summary, the word "Critical", the duration; an acknowledged problem has `is-acknowledged` and shows "acknowledged by anna" with the comment; **a host named `<img src=x onerror=alert(1)>` and a comment `<b>x</b> & "y"` appear as text and no `img` or `b` element exists**; a 200-character host name has a `title` with the full name.
  - `HostSquares.test.tsx`: one square per host with class by state, `title` = host name and state in words; with `max=3` and 5 hosts → 3 squares and "and 2 more".
  - `StateText.test.tsx`: the four sentences above.
  - `Dialog.test.tsx`: Escape closes; Tab stays inside; focus returns to the opener; `aria-modal` and a label.
  - `SecretDialog.test.tsx`: Copy writes the secret to the clipboard (mocked) and says "Copied"; the secret is not in the DOM after close.
  - `Pager.test.tsx`: "1–50 of 137"; Previous disabled on the first page; Next disabled on the last; total 0 renders nothing.
  - `source.test.ts`: no file under `src/` contains `dangerouslySetInnerHTML`, `innerHTML`, `eval(` or a colour literal (`#` followed by 3 or 6 hex digits) outside `tokens.css`.
- [ ] **Step 2: Run** `npm run check` in `ui/` — expected: fails.
- [ ] **Step 3: Implement** tokens (the twelve colours of concept section 7 with these names: `--bg`, `--panel`, `--raised`, `--line`, `--text`, `--dim`, `--critical`, `--high`, `--warning`, `--healthy-square`, `--healthy-text`, `--git`), base styles after `docs/ui/mockups/desk-views.html` (body 17 px/1.45, tables, buttons, inputs, focus outline), the font import, the components.
- [ ] **Step 4: Run** `npm run check` and `npm run build` — expected: pass, sizes within budget.
- [ ] **Step 5: Commit** `feat(ui): design tokens, base styles and shared components`

---

### Task 4: API client, session, routing and the shell

**Files:**
- Create: `ui/src/api/client.ts`, `ui/src/lib/router.ts`, `ui/src/lib/poll.ts`, `ui/src/lib/query.ts`, `ui/src/session.tsx`, `ui/src/shell/{Shell,LoginPage,PasswordDialog}.tsx`, `ui/src/pages/NotFoundPage.tsx`, tests beside each
- Modify: `ui/src/app.tsx`, `ui/src/api/types.ts` (`SessionUser`, `Status`, `SyncState`, `Listed<T>`)

**Interfaces:**
- Produces:

```ts
// api/client.ts
export class ApiError extends Error { status: number; code: string; }   // code "network" with status 0 when fetch itself fails
export const api: {
  get<T>(path: string, query?: Record<string, string | number | undefined>, signal?: AbortSignal): Promise<T>;
  send<T>(method: 'POST' | 'PUT' | 'PATCH' | 'DELETE', path: string, body?: unknown): Promise<T>;  // sends {} when body is undefined and the method is not DELETE; 204 → undefined
};
export function onUnauthorized(handler: () => void): void;   // called for any 401 except the login request itself

// lib/router.ts
export function useRoute(): { path: string; query: URLSearchParams };
export function navigate(to: string, opts?: { replace?: boolean }): void;
export function Link(props: { href: string; class?: string; children }): JSX.Element;  // plain <a>, intercepts unmodified left clicks
export function match(pattern: string, path: string): Record<string, string> | null;   // "/hosts/:name" → { name } decoded

// lib/query.ts
export function useQueryState(): [URLSearchParams, (changes: Record<string, string | undefined>) => void];  // setting a filter removes "offset"; uses replace

// lib/poll.ts
export function usePolled<T>(load: (signal: AbortSignal) => Promise<T>, deps: unknown[], everyMs?: number):
  { data: T | undefined; error: ApiError | undefined; staleSince: Date | undefined; reload: () => void };

// session.tsx
export type Role = 'viewer' | 'operator' | 'admin';
export function useSession(): { user: SessionUser; can: (role: Role) => boolean; logout: () => Promise<void> };
```

Behaviour:
- Paths: `/` redirects to `/dashboards`; `/dashboards`, `/hosts`, `/hosts/:name`, `/problems`, `/configuration`, `/administration`; anything else → `NotFoundPage` inside the shell. The pages of later tasks are placeholders with their heading for now.
- On start the app asks `GET /session`: `401` → login page; otherwise the shell. Login posts name and password; `401` shows the server's message; `429` shows "The server is busy, try again in a moment"; other failures "Could not reach the server". After login the user lands on the URL they asked for. The password field is cleared after every attempt. The login page links `/licenses.txt`.
- Any later `401` shows the login page with "Your session has ended. Log in again." and returns to the same URL afterwards.
- `usePolled`: loads at once, then every 10 s after the previous answer arrived; does not load while `document.hidden`, loads at once when it becomes visible; aborts on unmount and when `deps` change; keeps the last `data` on an error and sets `staleSince` to the time of the first failure; clears it on the next success. A `401` goes to `onUnauthorized`, not into `error`.
- Shell (concept 8.2, mockup): navigation Dashboards, Hosts, Problems with a badge of the unacknowledged count (sum over severities from `GET /status`, hidden at 0), Configuration, Administration (admin only); at the bottom the user name and role, "Change password" (dialog: current, new, repeat; client checks 12–256 characters and equality; `403` → "The current password is wrong"; success closes and says "Password changed") and "Log out". Top bar on every page from `GET /status` (polled): `In sync · <commit, 7 characters> · <age of last_success_at>` in the Git colour; with `gitops` true the note "Hosts, templates and dashboards are managed by Git"; with `status` `out_of_sync` a warning banner "Out of sync since <time of last_success_at>, showing the configuration from commit <7 characters>" and the first issue (`file:line:column — reason`) with "and N more", linking to `/configuration`; with `never` "The configuration has not been synchronised yet"; with `gitops` false no sync text. When the status poll is stale: banner "No connection to the server since <time>".
- A page for a higher role than the user's shows "Not allowed for your role" (`/administration` for non-admins).

- [ ] **Step 1: Write the failing tests**
  - `client.test.ts`: `get` builds the query and drops `undefined`; an error answer becomes `ApiError` with the body's code and message; a non-JSON error body → code `internal`; a rejected fetch → code `network`; `send` sends `Content-Type: application/json` and `{}` for a bodyless POST, nothing for DELETE; `204` → `undefined`; a `401` calls the handler once.
  - `router.test.ts`: `match('/hosts/:name', '/hosts/db%2D02.example')` → `{name: 'db-02.example'}`; no match for a longer path; `Link` with a plain click calls `navigate` and prevents default, with Ctrl held it does not; back button updates `useRoute`.
  - `query.test.ts`: setting `state` removes `offset`; `undefined` removes the key; uses `replaceState`.
  - `poll.test.ts` (fake timers): loads at once and again after 10 s; no load while hidden, one load on becoming visible; error keeps data and sets `staleSince`, success clears it; unmount aborts; changing deps drops a late answer of the old request.
  - `LoginPage.test.tsx`: success calls through and shows the shell; `401` shows the server's message and empties the password; `429` message; the form works with Enter.
  - `app.test.tsx`: start with `401` → login; after login the requested path `/problems?state=resolved` is shown; a `401` during a poll → login with "Your session has ended" and back to the same URL after login.
  - `Shell.test.tsx`: badge shows 3 for `critical.unacknowledged 2` + `warning.unacknowledged 1`, absent at 0; Administration link only for an admin; `/administration` as operator → "Not allowed for your role"; out-of-sync banner with file, position, reason and "and 2 more" for three issues; **an issue reason `<script>alert(1)</script>` is shown as text**; `never` text; `gitops: false` shows no sync text; stale status → "No connection to the server since".
  - `PasswordDialog.test.tsx`: too short and unequal are refused without a request; `403` message stays in the open dialog; success closes.
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm run check`, `npm run build` — expected: pass.
- [ ] **Step 5: Commit** `feat(ui): API client, session, routing and the desk shell`

---

### Task 5: Dashboards page

**Files:**
- Create: `ui/src/pages/DashboardsPage.tsx`, test beside it
- Modify: `ui/src/api/types.ts` (`DashboardSummary`, `Display`), `ui/src/test/fixtures.ts`

**Interfaces:**
- Consumes: `GET /dashboards`, `GET /displays` (both polled); `Link`, `Empty`, `usePolled`.
- Produces: the page at `/dashboards`.

Content (concept 8.3, C7):
- Table "Dashboards" in the server's order: name with the file below it; a strip of host squares built from `hosts_by_state` (counts → that many squares per class, capped at 40 with "and N more") and the host count; open problems in words ("2 critical, 1 warning", acknowledged ones not counted separately; "No problems" in the healthy text colour); priority. A row links to `/problems?dashboard=<id>`.
- Section "Wall displays": per display its name, its setting in a sentence — "Shows Kunde A – Region 2" or "Rotates through 4 dashboards every 30 s" — plus "switches on alarms from Critical" / "from Warning" / "never switches"; what it shows now: "Showing <dashboard name> because of 2 alarms" when `target` is set, with "Waiting: A, B" for `waiting`; and for admins nothing else here (tokens are on the Configuration page).
- No dashboards: "No dashboards yet. Add a file under `dashboards/` in the configuration repository." No displays: the section says "No wall displays configured."

- [ ] **Step 1: Write the failing tests:** order kept as delivered; squares strip counts (`critical 1, acknowledged 2, ok 3` → 6 squares with the right classes); "2 critical, 1 warning"; "No problems"; row link target with an id that needs encoding; display sentences for fixed, rotating, with and without `switch_on_alarm`; target sentence uses the dashboard's name, falls back to the id when the name is unknown; both empty states; a stale poll keeps the table and shows no spinner.
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm run check` — expected: pass.
- [ ] **Step 5: Commit** `feat(ui): dashboards page with wall displays`

---

### Task 6: Hosts list

**Files:**
- Create: `ui/src/pages/HostsPage.tsx`, test beside it
- Modify: `ui/src/api/types.ts` (`HostRow`)

**Interfaces:**
- Consumes: `GET /hosts` (`search`, `group`, `state`, `limit` 50, `offset`), `GET /host-groups`; `useQueryState`, `Pager`, `StateText`, `value`, `age`.
- Produces: the page at `/hosts`.

Content (concept 8.4): a search field (name or address; sent 300 ms after the last key stroke; at most 200 characters), a group select ("All groups" + the groups), a state select ("All", "With problems", "No data", "Without problems" → `problems`, `no_data`, `ok`). Columns: host with address below, groups, state (`StateText` in the severity colour), CPU, memory (`value`, a dash for `null`), last value (`age(last_seen_at)`). A row links to `/hosts/<name>`. With `metrics_available` false a banner: "Current values are not available right now." A search with no result: "No host matches." No hosts at all: "No hosts yet." When `total` drops below `offset` the page goes back to the last existing page.

- [ ] **Step 1: Write the failing tests:** the request carries the filters from the URL; typing waits 300 ms and then updates the URL and resets the offset; group and state selects; columns for a host with a problem, an acknowledged one, a healthy one, one that never reported (`never`), `cpu: null` → dash; the banner; both empty sentences; `total` 40 with `offset` 50 → the request is repeated with `offset` 0; row link for `db-02.example`.
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm run check` — expected: pass.
- [ ] **Step 5: Commit** `feat(ui): hosts list`

---

### Task 7: Host detail with graphs

**Files:**
- Create: `ui/src/pages/HostPage.tsx`, `ui/src/components/Graph.tsx`, `ui/src/components/AckDialog.tsx`, `ui/src/components/CloseDialog.tsx`, tests beside each
- Modify: `ui/src/api/types.ts` (`HostDetail`, `HostValue`, `Series`, `Threshold`), `ui/package.json` (uPlot)

**Interfaces:**
- Consumes: `GET /hosts/{name}` and `/values` (polled), `GET /hosts/{name}/series` (per graph, reloaded every 60 s), `POST /problems/{id}/ack`, `/close`, `POST /hosts/{name}/release`.
- Produces:

```ts
Graph(props: { points: [number, number][] | null; thresholds: Threshold[]; unitKey: string; height?: number })
AckDialog(props: { problem: Problem; onDone: (p: Problem) => void; onClose: () => void })      // optional comment, at most 1000 characters
CloseDialog(props: { problem: Problem; onDone: (p: Problem) => void; onClose: () => void })    // required comment, says what closing means
```

Content (concept 8.5):
- Head: name, address, groups, templates, defining file (`source_file`, or "Created by hand"), agent version ("unknown" for `null`), enrolled ("not enrolled" for `null`), the dashboards the host belongs to as links to `/problems?dashboard=<id>`, a pill "N open problems" in the worst severity or "No problems"; with `managed_by_git` the note "Managed by Git".
- Left: open problems as `ProblemRow` with "Acknowledge" (operators and admins; "Acknowledge again" when already acknowledged) and "Close" in a menu; then one graph per series: for every item of `items`, one graph when it has no parameter, and one per parameter value found in `/values` when it has one (a `disk.used_percent` host with thirty disks gets thirty graphs; graphs below the fold load when they scroll into view). A range switch 1 h / 6 h / 24 h / 7 d for the whole page, kept in the URL (`range`, default `6h`).
- Right: current values as a table (item with its parameter, value, age); triggers (name, expression, severity word, "Firing" in the severity colour or "Quiet").
- A link "Past problems of this host" → `/problems?state=resolved&host=<name>`.
- Admins: "Release agent binding" with a confirmation dialog ("The agent on this host must enrol again with a new key.").
- Graph: a line in the Git colour over time; each threshold a dashed horizontal line in its severity colour labelled with the trigger name; y-axis formatted with `value`; `points` null or empty → "No data in this range"; a `503` from the series route → "The metrics store did not answer" in the graph's place, the rest of the page stays. Unknown host (`404`) → "There is no host named <name>." with a link back to Hosts.
- A failed action (`403`, `409`, network) is shown inside its dialog, which stays open; `409` on acknowledge says "This problem is already resolved" and reloads the page data when the dialog is closed.

- [ ] **Step 1: Write the failing tests**
  - `Graph.test.tsx` (uPlot mocked at the module boundary; assert what is passed to it): series data from points; one dashed series or hook per threshold with the severity class; null and empty → the sentence, uPlot not created; unmount destroys the plot; a resize of the container resizes the plot.
  - `AckDialog.test.tsx` / `CloseDialog.test.tsx`: ack sends `{}` without a comment and `{comment}` with one; close refuses an empty comment without a request; `409` and `403` messages stay in the dialog; success calls `onDone` with the answer.
  - `HostPage.test.tsx`: head fields including "unknown" and "not enrolled", the dashboards links; a viewer sees no Acknowledge, Close or Release; an operator sees Acknowledge and Close but not Release; graphs: an item without a parameter requests `item=cpu.usage&range=6h`; `disk.used_percent` with values for `C:\` and `/` requests two series with `p_mount=C%3A%5C` and `p_mount=%2F`; changing the range updates the URL and every request; a `503` for one series shows the sentence there and the other graph is drawn; `404` page; values table and trigger states; the past-problems link for a host name with a dot.
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.** Create uPlot in an effect on a `div` ref; set sizes through its API (no inline style strings); load series for a graph only once it has been visible (`IntersectionObserver`, with a fallback that loads at once where it does not exist).
- [ ] **Step 4: Run** `npm run check`, `npm run build` — expected: pass, sizes within budget.
- [ ] **Step 5: Commit** `feat(ui): host detail with graphs, acknowledge and close`

---

### Task 8: Problems view

**Files:**
- Create: `ui/src/pages/ProblemsPage.tsx`, test beside it
- Modify: `ui/src/api/types.ts` (`ProblemsAnswer` with `dashboards`)

**Interfaces:**
- Consumes: `GET /problems` (`state`, `severity`, `host`, `group`, `dashboard`, `search`, `limit` 50, `offset`), `GET /dashboards` (for the filter's names), `GET /host-groups`; `AckDialog`, `CloseDialog`, `ProblemRow`, `Pager`, `useQueryState`.
- Produces: the page at `/problems`.

Content (concept 8.6): tabs "Open" and "Resolved" (`state`); filters: severity ("All", "Warning and above", "High and above", "Critical"), dashboard (names from `GET /dashboards`), group; a search field (300 ms, 200 characters); an active `host` filter is shown as a removable chip "Host: db-02". Columns: host (link to the host), problem (trigger, instance, summary) with the severity word, dashboards of the host (from the answer's `dashboards` map, as links; a dash for none), start (`TimeText`) and duration (to now for open, to `resolved_at` for resolved, with the reason in words: "ended by itself", "host removed", "trigger removed", "closed by <name>: <comment>"), action. Open problems: "Acknowledge" / "Acknowledge again" and "Close" for operators and admins; acknowledged rows dimmed with who, when and the comment. After an action the list reloads at once. Empty: "No open problems." / "No resolved problems match." An unknown dashboard in the URL (`400` from the server) shows the server's message with a link "Clear filters".

- [ ] **Step 1: Write the failing tests:** default request is `state=open`; the Resolved tab changes the URL and the request and resets the offset; each filter and the search reach the request; the host chip appears from `?host=` and removing it drops the parameter; dashboards column from the map, dash for an empty list; resolved row texts for the four reasons; a viewer has no action column; acknowledging through the dialog reloads the list; a `409` from the action stays in the dialog; the `400` message with "Clear filters"; both empty sentences; paging keeps the filters.
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm run check` — expected: pass.
- [ ] **Step 5: Commit** `feat(ui): problems view`

---

### Task 9: Configuration view

**Files:**
- Create: `ui/src/pages/ConfigurationPage.tsx`, test beside it
- Modify: `ui/src/api/types.ts` (`SyncHistoryEntry`, `UnknownAgent`, `EnrollmentKey`)

**Interfaces:**
- Consumes: `GET /status`, `GET /sync/history` (paged, 20 per page), `GET /unknown-agents` (operator), `GET/POST/DELETE /enrollment-keys` (admin), `GET /displays`, `POST/DELETE /displays/{id}/token` (admin); `SecretDialog`, `Dialog`, `Pager`.
- Produces: the page at `/configuration`.

Sections (concept 8.7), each hidden when the role may not read it:
1. **Synchronisation** (everyone): with `gitops` false: "GitOps is off. Hosts are managed with `cerbrix-server admin`." and nothing else in this section. Otherwise: state in words, the active commit in full with a copy button, last successful sync, last attempt; when out of sync, a table of all issues: file, position (`line:column`, or a dash), reason.
2. **Sync history** (everyone, GitOps on): commit (7 characters, full as tooltip), applied at, changes in words ("2 hosts changed, 1 dashboard added"; "No changes" when all lists are empty), expandable to the names.
3. **Unknown agents** (operator, admin): host name, first seen, last seen, attempts; empty: "No unknown agents."
4. **Enrolment keys** (admin): description, created, expires, state; "Create key" dialog (description 1–200 characters, validity select: 1 hour, 24 hours, 7 days, 30 days → `1h`, `24h`, `7d`, `30d`); the key is shown once in `SecretDialog` ("Give this key to the agent's installer."); "Revoke" with confirmation for active keys.
5. **Display tokens** (admin): per display its name, "Token created <time>" or "No token"; "Create token" (when one exists the confirmation says "The current token stops working at once."), shown once in `SecretDialog` ("Open /display/<id> on the wall monitor and enter this token."); "Revoke" with confirmation. A `404` on create shows "The configuration is being synchronised. Try again."

- [ ] **Step 1: Write the failing tests:** GitOps off text; in-sync fields; out-of-sync issue table with a `null` position as a dash and **a reason containing `<b>` shown as text**; history sentence for mixed changes and "No changes"; expanding shows names; a viewer sees sections 1–2 only and no request goes to the operator and admin routes; an operator sees 3; key creation sends `{description, expires_in}` and shows the key once, after closing the key is not in the DOM and the list reloads; description empty or 201 characters is refused without a request; revoke asks first; token create with an existing token shows the warning sentence; the `404` sentence; revoke token.
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm run check` — expected: pass.
- [ ] **Step 5: Commit** `feat(ui): configuration view with keys and display tokens`

---

### Task 10: Administration view

**Files:**
- Create: `ui/src/pages/AdministrationPage.tsx`, test beside it
- Modify: `ui/src/api/types.ts` (`User`)

**Interfaces:**
- Consumes: `GET/POST /users`, `PATCH /users/{id}`, `POST /users/{id}/password`, `DELETE /users/{id}`; `SecretDialog`, `Dialog`, `Pager`.
- Produces: the page at `/administration` (admins only).

Content (concept 8.7): a table of users: name (the own row marked "you"), role, state ("Active" / "Disabled"), created, password changed. "Add user" dialog: name (client check: 1–63 characters of `a-z 0-9 . _ -`, starting with a letter or digit), role, and a choice "Generate a password" (default) or "Set a password" (12–256 characters, entered twice). A generated password is shown once in `SecretDialog` ("Give this password to the user. They should change it after the first login."). Per row: change role (select), disable or enable, reset password (generate, shown once; or set), remove (confirmation naming the user). `409` messages: "This name is taken" on create; the server's message for the last enabled admin. Changing the own role, disabling or removing oneself asks "This ends your own session." first; a `429` shows "The server is busy, try again in a moment".

- [ ] **Step 1: Write the failing tests:** table rows and the "you" mark; create with a generated password shows it once and reloads; create with a set password sends it and shows no secret; invalid names and unequal passwords are refused without a request; `409` on create and on demoting the last admin stay in the dialog; role change sends `PATCH {role}`; disable and enable; reset shows the generated password; remove asks with the name and sends `DELETE`; the self-warning for the own row; `429` message; a non-admin gets "Not allowed for your role" and no request is sent.
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm run check` — expected: pass.
- [ ] **Step 5: Commit** `feat(ui): administration view for users`

---

### Task 11: Browser smoke run, documentation and release wiring

**Files:**
- Create: `ui/e2e/smoke.spec.ts`, `ui/playwright.config.ts`, `docs/web-ui.md`
- Modify: `scripts/e2e.sh`, `ui/package.json` (`@playwright/test` as a development dependency, script `smoke`), `.github/workflows/ci.yml` (only if the e2e job needs more time or a cache), `README.md`, `docs/install.md`, `docs/api.md` (one sentence that the web UI is served at `/`), `CONTRIBUTING.md`

**Interfaces:**
- Consumes: the running e2e stack with its admin user, its dashboard `e2e`, its display `e2e-wall` and its open problems.
- Produces: `npm run smoke` (reads `CERBRIX_UI_URL`, `CERBRIX_UI_USER`, and the password from the file named by `CERBRIX_UI_PASSWORD_FILE`).

- [ ] **Step 1: Write the smoke run** (one browser context, Chromium, no retries hidden from the output). It fails on any console error, any failed request to the server's own origin, and any content-security-policy violation. Steps: the login page loads and shows no placeholder text; a wrong password shows the message; login; the Dashboards page lists `E2E`; Hosts lists the e2e host and opening it draws at least one graph canvas with a non-zero size; reload on the host URL stays on the host page; Problems lists the unacknowledged problem, acknowledging it with a comment shows the user's name on the row; Configuration shows "In sync" and, creating a display token, the secret dialog (the value is not printed); Administration lists the admin; log out returns to the login page and a direct visit to `/hosts` shows the login page.
- [ ] **Step 2: Extend `scripts/e2e.sh`:** after the API walk-through and before clean-up, unless `CERBRIX_E2E_SKIP_UI=1`, run the smoke run in the Playwright Docker image (version pinned to the `@playwright/test` version in `package-lock.json`) attached to the compose project's network, addressing the server by its service name, with `ui/` mounted read-only and the password passed as a file under the script's `umask 077` work directory. The step must leave an unacknowledged alarm for nothing that follows (it runs last), print the Playwright report on failure with tokens masked like the rest of the script, and add no host dependency beyond Docker. Document the new variable in the script header.
- [ ] **Step 3: Write `docs/web-ui.md`:** what the UI is for and where it is (`https://<server>/`), the first login, the pages in one paragraph each, what each role sees, what refreshes by itself and how often, what "Out of sync" and "No connection to the server" mean, that secrets are shown once, supported browsers (current Firefox, Chrome, Edge, Safari), that there is one dark theme, and that wall displays follow in the next release. `docs/install.md`: the UI is part of the server binary and the Docker image; behind a reverse proxy pass every path to the server; building from source needs Node 22 for the UI. `README.md`: one paragraph and a link.
- [ ] **Step 4: Run** `npm run check`, `npm run build`, `cargo test --workspace`, `scripts/e2e.sh`, shellcheck and actionlint through their Docker images — expected: all pass, exit 0 each.
- [ ] **Step 5: Commit** `feat: browser smoke run in the end-to-end check, web UI documentation`
