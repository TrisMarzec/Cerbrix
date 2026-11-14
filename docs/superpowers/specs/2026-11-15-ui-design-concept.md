# Cerbrix – UI design concept

Date: 2026-11-15 · Status: approved by the project owner

Markers: **[D]** = decided by the project owner, **[A]** = author's call.

This document settles the look and structure of the web UI before it is built in M4 (issue #30). It extends the main design, `2026-11-14-cerbrix-design.md`; section 9 lists what it changes there.

Mockups (open the HTML files in a browser):

- `docs/ui/mockups/wall-dashboard-directions.html` – the wall dashboard in the two directions that were compared, and the Out Of Sync state. Direction A was chosen.
- `docs/ui/mockups/desk-views.html` – clickable desk views in direction A: dashboards, hosts, host detail, problems.

The mockups show the intended look, density and wording. Host names, numbers and panel choices in them are invented.

## 1. Intent

Cerbrix must be able to run all day on a wall monitor in an operations room, readable from several metres and without anyone touching it. **[D]** An operator needs many dashboards, for example one per customer and region, and must be able to create one quickly. **[D]** When an alarm fires on a dashboard the monitor is not showing, the monitor can switch to it by itself so people see what is wrong. **[D]**

The desk UI serves the same people when they sit down to look closer: find a host, read its graphs, acknowledge a problem.

## 2. Goals, non-goals, anti-goals

**Goals**

- A wall display mode without menus that needs no input for days. **[D]**
- Several dashboards, each defined by a JSON file in the configuration repository. **[D]**
- Position and size of every panel can be set in the file. **[D]**
- Automatic switching to a dashboard with an alarm, per display, with a priority order across dashboards. **[D]**
- Return from a switch when the alarm is acknowledged or resolved. **[D]**
- The Out Of Sync state is visible on every dashboard and every desk page, with file and reason.
- A desk UI that can be used without training (main design, goal).

**Non-goals**

- A dashboard editor in the UI. In the MVP GitOps is the only way to manage configuration; an editor belongs to "management in the UI with GitOps off" (v0.2).
- A light theme. Direction A is dark; a light variant for bright rooms can follow later. **[A]**
- Free-form query panels or a query language. A graph panel shows one item of one host.
- Sound, browser notifications, and switching the physical monitor's input.

**Anti-goals**

- A dashboard on which an alarm can go unnoticed because of how its author laid it out.
- A wall display that stays on an alarm nobody can clear from their desk.
- Colour used for decoration, so that red and yellow stop meaning "problem".

## 3. Constraints

- Everything in the main design holds: English UI, TypeScript single-page application embedded in the server binary, three roles, GitOps as an all-or-nothing switch.
- No server state only in process memory. What a display currently shows is derived from problems and configuration, not remembered by the server.
- A wall monitor is typically 1920×1080 or 3840×2160, viewed from 3 to 10 metres.

## 4. Approaches

### 4.1 What a dashboard is

| Approach | Description | Trade-off |
|---|---|---|
| Fixed layout | A dashboard is a name and a host selection; every dashboard looks the same | Created in seconds, always readable; no own graphs or arrangement |
| Free canvas with editor | Panels placed by drag and drop in the UI, as in Grafana | Most flexible; highest build effort, and contradicts Git as the only source in the MVP |
| **File with panels on a grid** | A JSON file with a host selection and an optional list of panels with position and size | Flexible without an editor; the author is responsible for a readable layout |

**Decision [D]:** file with panels on a grid. The owner wants dashboards in JSON and wants to set position and size. A file without panels gets a standard layout, so the quick case stays quick. **[A]**

### 4.2 Visual direction

| Direction | Description | Trade-off |
|---|---|---|
| **A: Board** | Dark. The problem list carries the screen like a departure board; colour appears only where something is wrong | Says at a glance *what* is broken; the whole screen is grid area. Needs a dim room or a good panel |
| B: Status | Light. One large coloured block with the count of critical problems, hosts as grouped tiles | Readable from furthest away; says only *that* something is wrong, and the block takes a quarter of the screen from the grid |

**Decision [D]:** A.

## 5. Configuration files

Two new file types in the configuration repository: `dashboards/*.json` and `displays/*.json`. Both get a published JSON Schema and take part in the all-or-nothing validation of the Git sync; an invalid file puts the installation Out Of Sync with file and reason like any other. **[A]**

### 5.1 Dashboard

```json
{
  "name": "Kunde A – Region 2",
  "priority": 20,
  "hosts": { "groups": ["kunde-a", "region-2"] },
  "panels": [
    { "type": "problems",    "x": 0, "y": 0, "w": 8, "h": 6 },
    { "type": "host-states", "x": 8, "y": 0, "w": 4, "h": 3 },
    { "type": "graph", "host": "db-02", "item": "disk.used_percent",
      "params": { "mount": "/" }, "range": "6h",
      "x": 8, "y": 3, "w": 4, "h": 2 },
    { "type": "value", "host": "web-01", "item": "load.1m",
      "x": 8, "y": 5, "w": 4, "h": 1 }
  ]
}
```

- **Identity [A]:** the file name without `.json` is the dashboard's id (used in URLs and in display files). `name` is the title shown. Both are unique.
- **`hosts` [A]:** selects the hosts the dashboard is about. `groups` lists groups a host must *all* belong to ("kunde-a" and "region-2"). An optional `names` list adds single hosts. An empty or missing `hosts` selects every host. A host may belong to any number of dashboards. **[D]**
- **`priority` [D]:** an integer, default 0. A higher number wins when several dashboards have alarms (section 6). The owner asked for a priority that is set in advance; that it is a number on the dashboard is **[A]**.
- **`panels` [D]:** optional. Each panel has `type`, `x`, `y`, `w`, `h` in grid cells.
- **Grid [A]:** 12 columns and 6 rows fill the screen below the header, so a dashboard never scrolls on a wall. `x` is 0–11, `y` is 0–5, and `x + w ≤ 12`, `y + h ≤ 6`. Cells, not pixels, so the same file works on Full HD and 4K.
- **Validation [A]:** the sync rejects panels that overlap or leave the grid, unknown panel types, a `graph` or `value` panel whose host is not selected by the dashboard or whose item key is not configured for that host (in its file or its templates), and duplicate ids or names. `params` are not validated, because instances such as mount points are only known at runtime; a panel whose instance does not exist shows "No data". Validation never depends on received values, so a sync result is determined by the repository alone.
- **Without `panels` [A]:** the standard layout: `problems` at 0,0 8×6 and `host-states` at 8,0 4×6.

**Panel types for the start [A]**

| Type | Shows | Extra fields |
|---|---|---|
| `problems` | Open problems of the selected hosts, most severe first, then newest; acknowledged ones dimmed at the end. Rows that do not fit are summarised as "and 7 more" | – |
| `host-states` | "40 of 44 without problems" and one square per host, coloured by its worst open problem | – |
| `graph` | History of one item of one host as a line. Every trigger on that item (and instance) that is a plain comparison adds a dashed threshold line in its severity colour, firing or not | `host`, `item`, `params` (optional), `range` (default `6h`) |
| `value` | The latest value of one item, large | `host`, `item`, `params` (optional) |

**Host state [A].** One rule for squares, the Hosts list and filters: a host's state is its worst open problem. Unacknowledged problems outrank acknowledged ones; a host whose open problems are all acknowledged keeps the severity colour at half strength, because it is still broken but someone has it. "No data" is not a separate mechanism: it is an open problem from a "no data for X" trigger and is coloured by that trigger's severity. A host without such a trigger never shows "No data" as a state.

The header (dashboard name, sync state, clock) and the switch band are part of the frame, not panels, so no layout can leave them out. **[A]**

### 5.2 Display

A display is one wall monitor.

```json
{
  "name": "NOC left",
  "show": { "rotate": ["kunde-a-region-1", "kunde-a-region-2", "kunde-b-region-1"], "every": "30s" },
  "switch_on_alarm": { "min_severity": "critical" }
}
```

- **`show` [A]:** either `{ "dashboard": "<id>" }` for a fixed dashboard or `{ "rotate": [ids], "every": "<duration>" }`. Rotation was offered by the author and not objected to.
- **`switch_on_alarm` [D]:** optional; without it the display never switches ("if wanted"). `min_severity` is one of the four severities, default `critical`. **[A]**
- **`dashboards` [A]:** optional list of dashboard ids the display may switch to; default is all. This keeps a monitor at customer A from ever showing customer B. When it is set, every dashboard named in `show` must be in it; the sync rejects the file otherwise, as it does for an id without a dashboard file.
- **Identity [A]:** the file name without `.json` is the display's id.
- **Access [A]:** a display signs in with a display token instead of a user. An admin creates the token in the Configuration view for a display that exists in the active configuration; it is stored as a hash, like enrolment keys, and can be revoked. The token is bound to the display id and grants read access to exactly the dashboards that display may show (its `show` targets and its switch targets), including the problems and values of the hosts those dashboards select, and nothing else. When the display file disappears or is renamed, the token is revoked, as an agent credential is when its host file disappears. The monitor opens `/display/<id>` once and enters the token; the browser keeps it.

## 6. Alarm switching

**Alarm:** an open problem that is not acknowledged, on a host selected by a dashboard, with a severity at or above the display's `min_severity`. **[A]**

Rules:

1. A display with `switch_on_alarm` always shows, among the dashboards it may switch to that have an alarm, the one with the highest `priority`. **[D]** Ties go to the more severe alarm, then to the older one. **[A]**
2. If a dashboard with a higher priority gets an alarm, the display switches on to it. An alarm on a lower one does not move it. **[A]**
3. When the shown dashboard has no alarm left, because its alarms were acknowledged or resolved **[D]**, the display goes to the next dashboard by rule 1, and when none is left, back to its `show` setting. Rotation resumes where it stopped. **[A]**
4. While switched, a band under the header says where the display came from and how many more dashboards are waiting ("2 more dashboards waiting"). The rows of the alarms that caused the switch are highlighted. **[A]**
5. A host in several dashboards raises an alarm on each of them; rule 1 decides which is shown. **[D]**
6. Acknowledging happens at a desk, by an operator or admin. The wall display takes no input. **[A]**

**Where it is decided [A].** The server computes, for a display, the alarm target (or none) and the queue of waiting dashboards. This is a pure function of open problems, acknowledgements, dashboards and the display file, so the server keeps no per-display state, and the desk UI can show the same answer without asking the monitor. Rotation is the browser's business: the timer and the position in the list live in the display's page, and displays report nothing back. Consequently "Wall displays" on the desk shows the alarm target while a display is switched, and otherwise its setting ("Rotates through 4 dashboards every 30 s"), not the dashboard currently in rotation.

## 7. Visual language (direction A) **[D]** direction, **[A]** details

**Colour.** Colour carries meaning and nothing else.

| Token | Value | Use |
|---|---|---|
| Background | `#10293b` | Page |
| Panel | `#173549` | Panels, tables |
| Raised | `#1d4059` | Selected navigation, buttons, hover |
| Line | `#2a4d63` | Row separators, borders |
| Text | `#eaf1f4` | Primary text |
| Dim | `#9db4c0` | Secondary text, labels, acknowledged problems |
| Critical | `#ff6352` | Severity `critical`, the switch band |
| High | `#ff9447` | Severity `high` |
| Warning | `#f5b83d` | Severity `warning`, the Out Of Sync band |
| Healthy square | `#2f6f6a` | Host squares without problems |
| Healthy text | `#6fcbb8` | "No problems" in desk tables |
| Git | `#8ed4ea` | Sync state, links, graph lines, focus outline |

Severity `info` uses Dim; it never raises an alarm colour. A healthy dashboard is nearly monochrome. Severity is never shown by colour alone: the word is always there too.

**Type.** One family, Barlow Semi Condensed (SIL Open Font License, shipped with the UI, no request to a font service). It is a signage face: narrow enough for long host names in large sizes and legible at a distance. Weights 400, 500, 600, 700. Numbers are tabular everywhere so columns do not jitter when values change. Labels are sentence case.

**Sizes on the wall.** All wall sizes scale with the screen width. Reference at 1920 px: dashboard name 60 px, host name in a problem row 48 px, problem text 38 px, clock 42 px, panel label 26 px. Nothing on the wall is smaller than 24 px at 1920.

**Problem row.** A coloured bar on the left edge, host name in the severity colour, problem text, severity and acknowledgement state in a small second line, duration on the right. The same row, smaller, is used in the desk tables, so a problem looks the same everywhere.

**Motion.** Only two things move unasked: a row that caused a switch fades in highlighted once, and a dashboard change is a short cross-fade. No blinking. `prefers-reduced-motion` turns both off.

**Out Of Sync.** On a wall dashboard, a warning-coloured band under the header: "Out Of Sync since 14:05, showing the configuration from commit 4f2a9c1", and below it the file, the position and the reason. With a switch band present, both are shown, switch band first. On desk pages the same message sits in the top bar and links to the Configuration view.

## 8. Views

### 8.1 Wall display (`/display/<id>`)

Full screen, no navigation, no cursor after a few seconds. Header: dashboard name left; sync state and clock right. Then the bands (switch, Out Of Sync) if any, then the 12×6 grid. If the connection to the server is lost, the header says "No connection to the server since 14:31" in the warning colour and the content stays but dims, so stale data is never mistaken for a quiet day. **[A]**

### 8.2 Desk shell

Navigation on the left: Dashboards, Hosts, Problems (with the count of unacknowledged problems), Configuration, Administration (admins only); user and role at the bottom. A top bar on every page: sync state with commit and age, and, with GitOps on, "Hosts, templates and dashboards are managed by Git". Below 860 px the navigation moves to the top.

### 8.3 Dashboards (start page) **[A]**

- A table of all dashboards: name and file, a strip of host squares with the host count, open problems in words, priority, and "Open on this screen", which shows the dashboard in wall mode in the current browser.
- Sorted like the switch logic: dashboards with alarms first, by priority.
- Below: "Wall displays" with each display's setting in a sentence and what it shows now, and the queue of waiting alarms per switching display.

### 8.4 Hosts

Search by name or address; filters for group and state (with problems, no data, without problems). Columns: host and address, groups, state in words, CPU (`cpu.usage`), memory (`memory.used_percent`), age of the last value. A host that does not collect one of the two items shows a dash there. Hosts with problems first. A row opens the host.

### 8.5 Host detail

- Head: name, address, groups, templates, defining file, agent version and enrolment date; a pill with the count of open problems.
- Left: open problems with "Acknowledge" (operator and admin), then history graphs per item with a range switch (1 h, 6 h, 24 h, 7 d). Multi-instance items get one graph per instance.
- Right: current values as a table of item and value; triggers with expression, severity and state (Firing or Quiet).
- Past problems of the host are a link to the Problems view filtered to this host.

### 8.6 Problems

Tabs Open and Resolved; filters for severity and dashboard; search. Columns: host, problem and severity, dashboards the host belongs to, start time and duration, action. "Acknowledge" opens a small dialog for an optional comment. Acknowledged problems are dimmed and show who acknowledged, when, and the comment.

### 8.7 Configuration and Administration

Not drawn; they use the same tables and top bar.

- **Configuration:** sync status (active commit, last successful sync, error of the last attempt with file, position and reason), sync history, unknown agents, enrolment keys (create, revoke), display tokens (create, revoke).
- **Administration:** users with role; create, change role, reset password, remove.

## 9. Changes to the main design

- 5.6 Repository layout gains `dashboards/*.json` and `displays/*.json`.
- 5.7 "Managed by Git" and the all-or-nothing validation cover dashboards and displays too.
- 5.9 "Dashboard" becomes "Dashboards": several, each with the content described here. The requirement that Out Of Sync is prominent applies to every one of them.
- 5.10 gains display tokens as a fourth kind of access, read-only and bound to one display.
- M2 scope grows by the schemas and validation for the two file types; M4 grows by the wall display mode, the panel grid and alarm switching. New issues are filed for these.

## 10. Left to the builder

- UI framework, component and chart libraries, build tooling.
- How the browser gets live updates (polling or a push channel), as long as a wall display reflects a new alarm within about 5 seconds and survives a server restart without a manual reload.
- The exact behaviour of rows that do not fit a `problems` panel beyond "summarise the rest".
- Spacing, radii and the precise type scale between the reference sizes given here.
- API shape between UI and server.

## 11. Later

- Light theme.
- Dashboard and display editor in the UI (with GitOps off).
- More panel types: text, host list, top-N by an item, a panel showing another dashboard's state.
- Per-customer access for users (today a display can be limited to dashboards, a user cannot).
