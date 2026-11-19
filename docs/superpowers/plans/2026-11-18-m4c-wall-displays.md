# M4c: Wall Displays Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A dashboard renders on its 12×6 panel grid in a wall mode without menus, and a wall monitor opened at `/display/<id>` shows its dashboard or rotation all day, switches to a dashboard with an alarm and back, and says so when the configuration is out of sync or the server cannot be reached.

**Architecture:** The wall mode is a second surface of the existing single-page application in `ui/`: `ui/src/wall/` holds a frame (header, bands, grid) and the four panel types, used by two entries — `/wall/<dashboard>` for a logged-in user ("Open on this screen") and `/display/<id>` for a wall monitor that authenticates with a display token kept in the browser. What a display shows is decided by a pure function over the server's display state (alarm target, waiting queue) and a rotation position that lives only in the page. The server gets two small additions to the display routes; everything else is client work on the API of M4a.

**Tech Stack:** as M4b — TypeScript (strict), Preact, Vite, Vitest with Testing Library and jsdom, uPlot; Rust (axum) for the two API additions; Playwright for one more step of the smoke run.

**Spec:** `docs/superpowers/specs/2026-11-15-ui-design-concept.md` (sections 5.1, 5.2, 6, 7 and 8.1, and 8.3 for "Open on this screen"). The API is `docs/api.md` (sections "Display tokens", "Dashboards", "Displays"). The intended look is `docs/ui/mockups/wall-dashboard-directions.html`, direction A (open it in a browser; its CSS is the reference for density and proportions).

## Decisions

| # | Decision | Why |
|---|---|---|
| D1 | Two entries: `/wall/<dashboard id>` uses the user's session and `GET /dashboards/{id}`; `/display/<display id>` uses a display token and the two display routes. Both render the same frame and panels | "Open on this screen" (concept 8.3) and the wall monitor (8.1) must look identical; only where the data comes from differs |
| D2 | The display token is kept in the browser's `localStorage` under `cerbrix.display.<id>` | The concept says the monitor enters the token once and the browser keeps it; a monitor has no user to log in again after a power cut. It is the only use of browser storage in the UI |
| D3 | A wall page polls every 5 seconds (the display state and the shown dashboard), with the same request time limit as the desk | The concept asks for a new alarm within about 5 seconds; the API has no push channel |
| D4 | Which dashboard a display shows is a pure function `whatToShow(state, rotation, now)`; the rotation position and timer live in the page and survive an alarm switch | Concept section 6: the server keeps no per-display state; rotation resumes where it stopped |
| D5 | All wall sizes derive from one CSS length `--u` = viewport width / 1920; nothing is smaller than `24 * --u` | Concept 7: sizes scale with the screen width, 24 px minimum at 1920; the same page works on 4K |
| D6 | How many problem rows fit a `problems` panel is computed from the panel's measured height and the row height; the rest is "and N more" (plus the server's `more`) | A wall never scrolls (concept 5.1) |
| D7 | When the page notices a new version of the UI on the server (the `ETag` of `/` changed) and the server is reachable, it reloads itself | "Survives a server restart without a manual reload" must include an upgrade: nobody presses F5 on a wall |
| D8 | With a display token the API returns problems without `acknowledged_by`, `ack_comment`, `closed_by` and `close_comment` (they are `null`); the wall shows "acknowledged" without a name | Issue #87: a monitor in a shared room must not show login names and comments; removing them on the server protects them from whoever holds the token |
| D9 | The display state gains `names`: the names of the dashboards in `allowed` | The switch band says where the display came from by name; a token cannot read the dashboards list |
| D10 | The switch band reads "Alarm: showing <dashboard>. <n> more dashboards waiting." and, when the display would otherwise show another dashboard, "Switched from <dashboard>." | Concept section 6 rule 4, in words that fit one line at 26 px |
| D11 | A wall page hides the mouse cursor after 3 seconds without movement; on movement a small "Back to the desk" link appears on `/wall/…` (never on `/display/…`) | Concept 8.1: no navigation, no cursor; a desk user needs a way back |
| D12 | `/display/<id>#token` shows the token form again | A monitor has no menu; this is the documented way to replace a token |
| D13 | Motion: a dashboard change is a 400 ms cross-fade; a row that caused a switch fades in highlighted once; both are off with `prefers-reduced-motion` | Concept section 7 |

## Global Constraints

- Everything in the M4b plan's Global Constraints holds (English, licences, content security policy, API text rendered as text, colours only from `tokens.css`, severity never by colour alone, tabular numbers, TypeScript strict, `npm run check` clean, `cargo` checks clean, size budget 150 kB gzip JavaScript and 30 kB gzip CSS).
- The wall takes no input: no control is needed to operate it, nothing is focusable in the frame, nothing scrolls. The only interactive things are the token form, and "Back to the desk" on `/wall/…`.
- The wall never shows stale data as current: after 15 seconds without a successful answer the header says "No connection to the server since <time>" in the warning colour and the content dims (concept 8.1).
- A wall page must run for days: every timer, observer and request is released when its component unmounts; one request per resource is in flight at most; a failed request never stops the loop.
- Wall sizes at 1920 px width (multiply by `--u`): dashboard name 60, host name in a problem row 48, problem text 38, clock 42, panel label 26; nothing below 24.
- The display token is never put in a URL, a log line or an error message; it is sent only as `Authorization: Bearer` to the two display routes.
- Tests that wait use fake timers or wait for a condition; none asserts after a fixed real sleep.
- Commits use explicit paths (`git commit -m "…" -- <paths>`); no formatter; single quotes as in the existing code.

## Review Focus

1. **Days of uptime** — timers, observers and uPlot instances are released on every dashboard change; polls do not pile up when the server is slow; memory does not grow with each rotation step. Tests in Tasks 2, 4 and 6.
2. **The server goes away and comes back** — restart, upgrade, network loss, a revoked token (`401`), a display removed from the configuration: the page says what is wrong, keeps the last picture dimmed where that is right, never goes blank or loops, and recovers by itself (including the reload on a new version). Tests in Task 6.
3. **Layouts their author got wrong** — a `problems` panel of one cell, seventy-two panels of one cell, a graph without data, a dashboard without hosts, a 200-character host name at 48 px, a hostile dashboard name: nothing overflows the screen, nothing is unreadable silently (too small to show anything → the panel says so). Tests in Tasks 2–4.
4. **Rotation meets alarms** — an alarm appears and clears mid-rotation; the target is not in the rotation; a rotation of one; `every` of 1 s; a dashboard of the rotation no longer exists; two alarms swapping priority between polls. Tests in Task 6 (`whatToShow`).
5. **Screens** — 1920×1080, 3840×2160 and a 1366×768 laptop for "Open on this screen"; `prefers-reduced-motion`. A CSS-reading test for the scale rule and the 24 minimum in Task 2; the controller checks in a browser.

## File Structure

```
ui/src/wall/
  wall.css            the scale unit, wall type sizes, frame, bands, grid, panels
  WallFrame.tsx       header (name, sync state, clock), bands, dimming, cursor hiding
  PanelGrid.tsx       places panels on the 12×6 grid
  panels/ProblemsPanel.tsx, HostStatesPanel.tsx, GraphPanel.tsx, ValuePanel.tsx
  fit.ts              rowsThatFit, squaresThatFit (pure)
  show.ts             whatToShow, rotation helpers, parseEvery (pure)
  displayClient.ts    fetch with the bearer token, time limit, clock skew; token storage
  WallPage.tsx        /wall/<dashboard>: session data source
  DisplayPage.tsx     /display/<id>: token form, state poll, switching, reload on a new version
ui/src/app.tsx        chooses desk shell, wall page or display page by path
crates/cerbrix-server/src/api/ui/displays.rs   names, privacy for token access
```

---

### Task 1: API additions for the wall (names of allowed dashboards, no user names for a token)

**Files:**
- Modify: `crates/cerbrix-server/src/api/ui/displays.rs`, `crates/cerbrix-server/src/view/mod.rs` (the problem view) or where the dashboard answer is built, `docs/api.md`

**Interfaces:**
- Produces (JSON):
  - `GET /api/v1/display/{id}`: additional `"names": {"<dashboard id>": "<name>"}` with one entry for every id in `allowed` that exists.
  - `GET /api/v1/display/{id}/dashboards/{dashboard}` **when the request is authenticated by a display token**: every problem in the answer has `acknowledged_by`, `ack_comment`, `closed_by` and `close_comment` set to `null`; `acknowledged_at` and everything else is unchanged. With a user session the answer is unchanged.

- [ ] **Step 1: Write the failing tests** (router tests in `displays.rs`): `display_state_names_its_allowed_dashboards` (two allowed, one of them removed from the configuration → one entry); `token_sees_no_user_names` (an acknowledged problem with a comment, fetched with the token → the four fields are `null`, `acknowledged_at` is set; in every panel that carries problems and in `hosts[].state` if it carries any of the four); `user_still_sees_user_names` (same request with a session); `names_cost_no_extra_query_per_dashboard` (ten allowed dashboards: the state route's store calls stay the fixed set — assert by reading the handler in review; the test asserts the answer).
- [ ] **Step 2: Run** `cargo test -p cerbrix-server api::ui::displays` — expected: the new tests fail.
- [ ] **Step 3: Implement.** The names come from the dashboards the handler already loads (`board_inputs`); no new query. Removing the names is done on the answer for token access in one place (a function over the dashboard answer), not by a second serializer.
- [ ] **Step 4: Update `docs/api.md`** (the display state table and example; a paragraph under "Display tokens": what a token cannot read) and run `cargo test -p cerbrix-server` — expected: all pass, `every_route_is_documented` included.
- [ ] **Step 5: Commit** `feat(server): dashboard names for a display, no user names for a display token`

---

### Task 2: Wall frame and panel grid

**Files:**
- Create: `ui/src/wall/wall.css`, `ui/src/wall/WallFrame.tsx`, `ui/src/wall/PanelGrid.tsx`, `ui/src/wall/fit.ts`, tests beside each
- Modify: `ui/src/api/types.ts` (`DashboardDetail`, `Panel` union with the four types, `Placement`, `SyncState` if not exported)

**Interfaces:**
- Produces:

```ts
// api/types.ts — exactly the answer of GET /dashboards/{id} in docs/api.md
export type Panel = ProblemsPanelData | HostStatesPanelData | GraphPanelData | ValuePanelData; // each with type, x, y, w, h
export interface DashboardDetail { id: string; name: string; file: string; priority: number; hosts: { name: string; state: HostState | null }[]; panels: Panel[]; metrics_available: boolean }

// wall/WallFrame.tsx
export interface WallAlarm { showing: string; switchedFrom?: string; waiting: number }
WallFrame(props: {
  title: string;                         // dashboard name
  sync?: SyncState;                      // undefined: no sync text (GitOps off or unknown)
  now: Date;
  alarm?: WallAlarm;                     // the switch band
  lostSince?: Date;                      // "No connection to the server since …", dims the content
  back?: string;                         // href of "Back to the desk"; undefined on a display
  children: ComponentChildren;
})

// wall/PanelGrid.tsx
PanelGrid(props: { panels: Panel[]; render: (panel: Panel, index: number) => ComponentChildren })

// wall/fit.ts
export function rowsThatFit(panelHeightPx: number, rowHeightPx: number, labelHeightPx: number): number; // ≥ 0; one row is reserved for "and N more" when not all fit
export function squaresThatFit(widthPx: number, heightPx: number, squarePx: number, gapPx: number): number;
```

Frame content (concept 7 and 8.1): header with the title left and, right, the sync state ("In sync", or "Out of sync" in the warning colour with the word) and the clock (`HH:MM`); below it the bands in this order: the switch band (critical colour; text per D10), the Out Of Sync band (warning colour): "Out of sync since <time>, showing the configuration from commit <7 characters>" and below it the first issue as `file:line:column — reason` with "and N more"; then the grid, filling the rest of the viewport exactly (the page never scrolls: `height: 100vh; overflow: hidden`). With `lostSince`: the header shows "No connection to the server since <time>" in the warning colour in place of the sync state, and the grid gets the class that dims it. The cursor is hidden after 3 s without mouse movement (class on the frame; any movement shows it and the `back` link for 3 s).

Grid: CSS grid of 12 columns and 6 rows with a gap; a panel is placed at `x + 1 / span w`, `y + 1 / span h`. Panels whose placement is outside the grid or overlaps are not possible (the configuration compiler rejects them); a defensive clamp keeps a bad value inside the grid.

- [ ] **Step 1: Write the failing tests**
  - `fit.test.ts`: `rowsThatFit` — exact fit (no reserve needed is the caller's business: document it), height below one row → 0, negative and NaN → 0; `squaresThatFit` for a few sizes, zero area → 0.
  - `wall.css` read from disk (as `wrapping.test.ts` does): `--u` is defined as `calc(100vw / 1920)`; every `font-size` in the file is `calc(var(--u) * N)` with N ≥ 24; the frame has `height: 100vh` and `overflow: hidden`; a `prefers-reduced-motion` block turns transitions and animations off.
  - `WallFrame.test.tsx`: title and clock; sync texts for `in_sync`, `out_of_sync` (band with commit, first issue, "and 2 more"), `never`, and `sync` undefined (no sync text); the switch band texts for waiting 0, 1 and 2 and with and without `switchedFrom`; band order (switch first); `lostSince` replaces the sync state and the content region has the dim class; a hostile title and reason shown as text; the cursor class appears after 3 s (fake timers) and goes on `mousemove`; the back link exists only with `back` and nothing else in the frame is focusable.
  - `PanelGrid.test.tsx`: placement styles for a panel at `x 8, y 3, w 4, h 2`; 72 one-cell panels render; a panel with `x 11, w 4` is clamped to the grid.
- [ ] **Step 2: Run** `npm run check` in `ui/` — expected: fails.
- [ ] **Step 3: Implement.** Grid placement is set through the DOM style object (allowed by the content security policy), sizes through CSS classes and `--u`.
- [ ] **Step 4: Run** `npm run check`, `npm run build` — expected: pass, sizes within budget.
- [ ] **Step 5: Commit** `feat(ui): wall frame and panel grid`

---

### Task 3: Problems and host-states panels

**Files:**
- Create: `ui/src/wall/panels/ProblemsPanel.tsx`, `ui/src/wall/panels/HostStatesPanel.tsx`, tests beside each
- Modify: `ui/src/wall/wall.css`

**Interfaces:**
- Consumes: `rowsThatFit`, `squaresThatFit`, `problemName`, `severityWord`, `duration` from the existing code.
- Produces:

```ts
ProblemsPanel(props: { panel: ProblemsPanelData; now: Date; causes: string[] })   // causes: ids of the alarms that caused a switch
HostStatesPanel(props: { panel: HostStatesPanelData })
```

Problems panel (concept 5.1, 7): label "Problems"; one row per problem in the server's order: a coloured bar on the left, the host name in the severity colour (48), the problem text — trigger, instance, summary — (38), a second line with the severity word and "acknowledged" when acknowledged (26), the duration on the right (38). Acknowledged rows are dimmed (half strength) and, as the server orders them, come last. A row whose id is in `causes` has the highlight class (fades in once). Long text is cut with an ellipsis on one line (a wall row never wraps to more than its two lines). Rows that do not fit are replaced by one last line "and N more" where N = problems not shown + the panel's `more`. No problems: "No open problems" in the healthy text colour, large. The panel measures its body with a `ResizeObserver` and its row height from a hidden sample row.

Host-states panel: label "Hosts"; the sentence "40 of 44 without problems" (38; "All 44 without problems" in the healthy text colour when none has one; "No hosts" for an empty selection); one square per host coloured by state (`sev-…`, `is-acknowledged`, `is-ok`), problem hosts first as the server orders them, as many as fit and then "and N more"; the panel has a text alternative with the counts per severity in words.

A panel too small to show its first row or the sentence shows only its label.

- [ ] **Step 1: Write the failing tests** (the `ResizeObserver` and element heights are mocked; a helper in `ui/src/wall/test/measure.ts` sets them): rows in the server's order with host, text, severity word, duration; "acknowledged" without a name even when the data has one; the highlight class exactly on the `causes` rows; 10 problems with room for 4 rows → 3 rows and "and 7 more"; with `more: 5` → "and 12 more"; room for 0 rows → the label only; no problems → "No open problems"; a 200-character host name has the ellipsis class and a `title`; a hostile summary is text; the observer is disconnected on unmount. Host states: the three sentences; squares with classes; 500 hosts with room for 120 → 119 squares and "and 381 more"; the text alternative.
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm run check` — expected: pass.
- [ ] **Step 5: Commit** `feat(ui): problems and host-states panels for the wall`

---

### Task 4: Graph and value panels

**Files:**
- Create: `ui/src/wall/panels/GraphPanel.tsx`, `ui/src/wall/panels/ValuePanel.tsx`, tests beside each
- Modify: `ui/src/components/Graph.tsx` (wall sizing through optional props), `ui/src/wall/wall.css`

**Interfaces:**
- Produces:

```ts
GraphPanel(props: { panel: GraphPanelData; now: Date })
ValuePanel(props: { panel: ValuePanelData; now: Date })
// components/Graph.tsx: additional optional props, defaults unchanged for the desk
//   fill?: boolean        take the height of the container (ResizeObserver) instead of `height`
//   fontPx?: number       axis and label font size in px (the wall passes 24 * u)
//   interactive?: boolean default true; false: no cursor, no readout line
```

Graph panel (concept 5.1): label "<host> · <item> · <params>" (26); the line and the dashed threshold lines with their trigger names as on the desk, sized for the wall (axis text 24·u, line width scaled), filling the panel; no hover readout; `points` null or empty → "No data" (38, dim); the latest value at the top right of the panel (38) through `value(item, v)`.

Value panel: label as above; the latest value large (the largest size that fits the panel's height, at most 120·u, at least 38·u) through `value(item, v)`, and below it the age ("2 min ago") in dim text (26); `value` null → "No data".

- [ ] **Step 1: Write the failing tests** (uPlot mocked as in `Graph.test.tsx`): `Graph` with `fill` sizes the plot from the container and follows a resize; with `interactive: false` no cursor option and no readout element; `fontPx` reaches the axis fonts; the desk defaults are unchanged (existing tests stay green). Graph panel: label with and without params; latest value from the last point; null and empty points → "No data" and no plot; thresholds passed through; destroyed on unmount and when the panel's series identity (host, item, params, range) changes; new points for the same identity use `setData`. Value panel: value and age; null → "No data"; the size class steps for a 1-row and a 3-row panel (height mocked).
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.** `u` in pixels is read once per resize from the frame (`window.innerWidth / 1920`).
- [ ] **Step 4: Run** `npm run check`, `npm run build` — expected: pass.
- [ ] **Step 5: Commit** `feat(ui): graph and value panels for the wall`

---

### Task 5: "Open on this screen" — a dashboard in wall mode for a logged-in user

**Files:**
- Create: `ui/src/wall/WallPage.tsx`, `ui/src/wall/DashboardView.tsx` (frame + grid + panels from one `DashboardDetail`; used by both entries), tests beside each
- Modify: `ui/src/app.tsx` (path split), `ui/src/pages/DashboardsPage.tsx` (the link), `ui/src/shell/Shell.tsx` only if the path split needs it

**Interfaces:**
- Consumes: `WallFrame`, `PanelGrid`, the four panels, `usePolled`, `useSession`, `api`.
- Produces:

```ts
// wall/DashboardView.tsx
DashboardView(props: { dashboard: DashboardDetail; sync?: SyncState; now: Date; alarm?: WallAlarm; causes: string[]; lostSince?: Date; back?: string })
// app.tsx: '/display/…' → DisplayPage (Task 6, a placeholder now); inside the session: '/wall/:id' → WallPage, everything else → Shell
```

`WallPage` at `/wall/<dashboard id>`: inside the session (not logged in → the login page, then back to the same URL), outside the shell (no navigation, no top bar). It polls `GET /dashboards/{id}` and `GET /status` every 5 s; the clock is the server-corrected `now`, refreshed every 5 s (it shows minutes). `lostSince` is set when the dashboard poll has had no successful answer for 15 s. `404` → a plain page "There is no dashboard named <id>." with a link back to `/dashboards`. Before the first answer: the empty frame with the id as title. `back` is `/dashboards`. The document title is "<dashboard name> · Cerbrix".

Dashboards page: each row gets a link "Open on this screen" to `/wall/<id>` (the row's name link to the problems stays).

- [ ] **Step 1: Write the failing tests:** `app.test.tsx` — `/wall/kunde-a` renders no navigation and requests the dashboard; `/display/x` renders the placeholder without requesting `/session`; `/hosts` still renders the shell. `WallPage.test.tsx` — panels of all four types from a fixture taken from `docs/api.md`; polls every 5 s; a failed poll keeps the picture, and after 15 s without success the header says "No connection to the server since" and the grid is dimmed, and a later success clears it; `404` page; out-of-sync band from `/status`; the back link; nothing of the previous dashboard is shown after navigating from `/wall/a` to `/wall/b` (keyed). `DashboardsPage.test.tsx` — the link per row with an id that needs encoding.
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm run check`, `npm run build` — expected: pass.
- [ ] **Step 5: Commit** `feat(ui): open a dashboard in wall mode on this screen`

---

### Task 6: The wall display page — token, rotation, alarm switching

**Files:**
- Create: `ui/src/wall/show.ts`, `ui/src/wall/displayClient.ts`, `ui/src/wall/DisplayPage.tsx`, tests beside each
- Modify: `ui/src/app.tsx` (replace the placeholder), `ui/src/api/types.ts` (`DisplayState`), `ui/src/wall/wall.css` (cross-fade, token form)

**Interfaces:**
- Produces:

```ts
// wall/show.ts — pure
export function parseEvery(text: string): number | undefined;            // "30s", "5m", "1h", "1m30s" → seconds; anything else → undefined
export interface Rotation { index: number; since: number }               // position in the rotation list and when it was reached (ms)
export interface Shown { dashboard: string | undefined; alarm?: { switchedFrom?: string; waiting: number }; rotation: Rotation }
export function whatToShow(state: DisplayState, rotation: Rotation, nowMs: number, missing: ReadonlySet<string>): Shown;

// wall/displayClient.ts
export function storedToken(displayId: string): string | undefined;
export function storeToken(displayId: string, token: string): void;
export function forgetToken(displayId: string): void;
export function displayGet<T>(displayId: string, path: string, signal?: AbortSignal): Promise<T>;  // '' for the state, '/dashboards/<id>' for a dashboard; Bearer token when one is stored; 15 s limit; ApiError as on the desk; records the clock skew from the Date header
```

`whatToShow` rules (concept section 6):
- The rotation advances when `nowMs - rotation.since ≥ every` and no alarm target is set; it wraps; entries in `missing` (dashboards that answered 404) are skipped; a rotation whose entries are all missing shows `undefined`. `every` unparsable or below 5 s counts as 5 s. A fixed display has a rotation of one.
- With `state.target` set, that dashboard is shown and the rotation does not advance (`since` is pushed forward so that the interrupted dashboard gets its full time after the return); `alarm.switchedFrom` is the rotation's current dashboard when it differs from the target; `alarm.waiting` is `state.waiting.length`.
- When the target clears, the rotation's current dashboard is shown again.
- A target that is not in `state.allowed` is ignored (defensive; the server never sends one).

`DisplayPage` at `/display/<display id>` (no session, no shell):
- No token stored and the state request answers `401` → the token form: a heading "Wall display <id>", one field "Display token", a button "Start"; a wrong token → "This token is not valid for this display." A logged-in user needs no token (the request succeeds with the session cookie). `#token` in the address always shows the form (D12).
- State poll every 5 s; the shown dashboard is fetched every 5 s and at once when `whatToShow` changes; two `DashboardView`s are kept only during the 400 ms cross-fade.
- `causes` from the state are passed to the problems panels. Names for the band come from `state.names`.
- `401` after a working token (revoked or replaced) → the token form with "The token of this display is no longer valid." `404` for a user session → "There is no display named <id>." A dashboard that answers `404` is added to `missing`; one that answers `403` shows "This display may not show <id>." in the frame.
- Connection loss as in Task 5 (15 s), with the last picture kept and dimmed; recovery clears it.
- New version: every 60 s a `HEAD /` (same origin, no credentials needed); when its `ETag` differs from the one seen at start and the last state poll succeeded, `location.reload()`.
- The clock and all durations use the server-corrected time.
- The document title is "<display name> · Cerbrix".

- [ ] **Step 1: Write the failing tests**
  - `show.test.ts`: `parseEvery` table; fixed display; rotation advancing and wrapping exactly at `every`; target set → shown, rotation frozen; target equal to the rotation's current dashboard → no `switchedFrom`; target cleared → back to the interrupted dashboard with a full interval; target changes from A to B between polls; rotation of one; `every` "1s" → 5 s; a missing dashboard skipped; all missing → undefined; a target outside `allowed` ignored; deterministic for the same inputs.
  - `displayClient.test.ts`: the bearer header only when a token is stored; never a token in the URL; 15 s limit → `network`; storage functions; a storage that throws (private mode) → the token works for the session of the page and a notice says it cannot be kept.
  - `DisplayPage.test.tsx` (fake timers, `mockFetch`): token form on `401`, a wrong token, a right token is stored and the dashboard appears; a logged-in user sees the dashboard without a form; `#token` shows the form; rotation through two dashboards on the interval; an alarm target switches at the next poll, shows the band with the name from `names` and the waiting count, highlights the cause rows, and returns after it clears; `401` later → the form with the "no longer valid" text and the stored token removed; the `404` and `403` texts; connection loss after 15 s and recovery; a changed `ETag` reloads (the reload function is injected) and an unchanged one does not, and no reload while the connection is lost; unmount clears every timer and aborts requests (no request after unmount); the token never appears in any requested URL.
- [ ] **Step 2: Run** `npm run check` — expected: fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm run check`, `npm run build` — expected: pass, sizes within budget.
- [ ] **Step 5: Commit** `feat(ui): wall display page with rotation and alarm switching`

---

### Task 7: Desk links, smoke run, documentation

**Files:**
- Modify: `ui/src/pages/ConfigurationPage.tsx` (token note), `ui/src/pages/DashboardsPage.tsx` (display link), `ui/e2e/smoke.spec.ts`, `scripts/e2e.sh` (only if the walk-through's state must change), `docs/web-ui.md`, `docs/install.md`, `docs/configuration-repository.md`, `README.md`

**Interfaces:**
- Consumes: everything above; the e2e stack with dashboard `e2e` and display `e2e-wall`.

- [ ] **Step 1: Desk changes with tests:** the token note reads "Open /display/<id> on the wall monitor and enter this token."; in "Wall displays" on the Dashboards page each display's name links to `/display/<id>`.
- [ ] **Step 2: Extend the smoke run** with two steps, before the step that acknowledges the open problem: (a) logged in, open `/wall/e2e`: no navigation, the dashboard's name in the header, a problems panel with the unacknowledged problem, a graph canvas with a non-zero size, the page does not scroll (`scrollHeight == clientHeight`), then back with the "Back to the desk" link; (b) in a second browser context without cookies, open `/display/e2e-wall`: the token form; a wrong token shows the message; with the token created in the Configuration step (keep it in a variable, never print it) the dashboard appears, the switch band says "Alarm", and the problem row is highlighted. The existing listeners (console errors, failed requests, content security policy) apply to both; the expected `401` of the tokenless request is allowed explicitly. Reorder the existing steps as needed and say so.
- [ ] **Step 3: Documentation:** `docs/web-ui.md` — a section "Wall displays": what a display is, setting one up (the display file, the token in Configuration, opening `/display/<id>` on the monitor, entering the token once, `#token` to replace it), what the monitor shows (rotation, alarm switching and the band, Out of sync, no connection, the reload after an upgrade), that it shows no user names, "Open on this screen", a browser in kiosk mode (one example command line each for Chromium and Firefox), 5-second refresh. `docs/install.md`: one paragraph and the link. `docs/configuration-repository.md`: a link from the dashboards and displays section. `README.md`: one sentence. Remove "wall displays follow in the next release" wherever it stands.
- [ ] **Step 4: Run** `npm run check`, `npm run build`, `cargo test --workspace`, `scripts/e2e.sh`, shellcheck and actionlint — expected: all pass.
- [ ] **Step 5: Commit** `feat: wall displays in the desk UI, smoke run and documentation`
