# The configuration repository

With GitOps enabled, the whole monitoring configuration of a Cerbrix installation lives as JSON
files in a Git repository: the hosts, their items and triggers, templates, notification rules,
dashboards and displays. The server fetches the repository regularly, checks it and applies it.
How to connect the server to the repository is described in
[Configuration from Git](install.md#configuration-from-git). This page describes what the
repository contains.

A complete, valid repository is in [`examples/config-repo/`](../examples/config-repo/). The
examples below are valid as well; the item keys are listed in [items.md](items.md).

## Layout

```
hosts/            one file per host (subdirectories are allowed)
templates/        reusable sets of items and triggers
notifications/    who is told about problems
dashboards/       which hosts and panels a dashboard shows
displays/         what a screen shows, and when it switches dashboards
```

- Only files ending in `.json` below these five directories are read. Everything else in the
  repository (a `README.md`, a `.github/` directory, ...) is ignored.
- Files are UTF-8 without a byte order mark. Unknown fields are an error, so a misspelled
  field is found instead of silently ignored. Do not add a `$schema` key to the files; use the
  editor setting described in [Schemas in the editor](#schemas-in-the-editor).
- The identifier of a dashboard or display is its file name without `.json`
  (`dashboards/production.json` is `production`). Hosts, templates and notifications are
  identified by the name inside the file, not by the file name.
- Limits: a file may be at most 1 MiB; the five directories may hold at most 20,000 files
  together (all files count, not only `.json` files) and 64 MiB of configuration files;
  directories may be nested at most 16 levels deep. A dashboard may have at most 72 panels, a
  host may list at most 32 templates. Symbolic links and Git submodules are not allowed. Each of
  these makes the revision invalid.
- No string in a file, neither a key nor a value, may contain a control character (for example
  NUL, a tab or a line break, written as `\u0000`, `\t` or `\n`), and no file or directory name
  in the five directories may contain one.
- The server reads one branch (default `main`), the newest commit only. There is no history to
  manage: to undo a change, revert the commit.

Intervals and durations are written as digits followed by `s`, `m`, `h` or `d` (`"30s"`,
`"5m"`, `"1h"`), from `1s` to `24h`.

## Host files (`hosts/`)

```json
{
  "host": "web-01",
  "address": "10.0.4.12",
  "groups": ["production", "web"],
  "templates": ["linux-base"],
  "items": [
    { "key": "cpu.usage", "interval": "10s" },
    { "key": "disk.used_percent", "params": { "mount": "/var" }, "interval": "1m" },
    { "key": "net.rx_bytes_per_sec", "params": { "interface": "eth0" }, "interval": "30s" }
  ],
  "triggers": [
    {
      "name": "Var almost full",
      "expression": "disk.used_percent{mount=/var} > 85 for 5m",
      "severity": "warning"
    },
    {
      "name": "CPU saturated",
      "expression": "cpu.usage >= 95 for 15m",
      "severity": "high"
    }
  ]
}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `host` | yes | The host name. This is the name the agent uses in its `host` setting. Letters, digits, `.`, `_` and `-`, starting with a letter or digit, at most 253 characters. Unique in the repository. |
| `address` | no | Free text, for example the IP address. Shown by `admin host show`. |
| `groups` | no | Group names (see [Names](#names)). Dashboards and notifications select hosts by group. |
| `templates` | no | Names of templates to apply, in order, at most 32. Each must exist and may be listed once. |
| `items` | no | Items the host collects, see below. |
| `triggers` | no | Trigger definitions, see below. |

An item entry has `key` (a [built-in item](items.md)), `interval`, and `params` where the item
takes one: `mount` for `disk.*` items and `interface` for `net.*` items; other items take none.
A parameter value must not be empty and at most 256 bytes long. A host collects only the items
that its files list, directly or through templates; there are no hidden defaults.

A trigger entry has a `name`, an `expression` and a `severity`, one of `info`, `warning`,
`high`, `critical`. See [Trigger expressions](#trigger-expressions). What the server does with a trigger (problems, e-mails, no-data detection) is described in [Alerting](alerting.md).

## Template files (`templates/`)

```json
{
  "template": "linux-base",
  "items": [
    { "key": "cpu.usage", "interval": "30s" },
    { "key": "memory.used_percent", "interval": "30s" },
    { "key": "disk.used_percent", "interval": "5m" }
  ],
  "triggers": [
    {
      "name": "Disk almost full",
      "expression": "disk.used_percent > 90 for 10m",
      "severity": "high"
    },
    {
      "name": "Agent silent",
      "expression": "no data for 3m",
      "severity": "critical"
    }
  ]
}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `template` | yes | The template name. Unique. |
| `items` | no | Same as in a host file. |
| `triggers` | no | Same as in a host file. |

### How templates are merged into a host

1. The templates are applied in the order of the host's `templates` list. If two templates
   define the same item or trigger, the later one wins.
2. Then the host's own items and triggers are applied. They win over the templates.
3. An item is identified by its key together with its parameters, so `disk.used_percent` for
   `/` and for `/var` are two items, and `disk.used_percent` without `params` is a third one
   (an empty `params` object means the same as none). Two entries with the same identity
   replace each other: the interval of the later one is used.
4. A trigger is identified by its name. A host trigger with the name of a template trigger
   replaces it.

In the example above, `web-01` collects `cpu.usage` every 10 s (its own entry replaces the
template's 30 s), `memory.used_percent` every 30 s, `disk.used_percent` without parameters
every 5 min from the template, plus the `/var` and `eth0` items. `admin host show <name>` prints
the result of the merge, with the origin (`host` or `template:<name>`) of every item and trigger.

## Notification files (`notifications/`)

```json
{
  "name": "ops",
  "email": ["ops@example.com", "oncall@example.com"],
  "min_severity": "high",
  "groups": ["production"]
}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `name` | yes | Unique name. |
| `email` | yes | At least one address, each with exactly one `@` and text on both sides. |
| `min_severity` | yes | Problems of this severity or higher are sent. |
| `groups` | no | Group names; the rule applies to hosts in these groups. |

A rule applies to a problem when its severity is at least `min_severity` and the host is in all the listed groups (no `groups`: every host). How the e-mails look and what happens when the mail server is down is described in [Alerting](alerting.md#who-is-told-notification-rules); the SMTP server itself is set up in [E-mail notifications](install.md#e-mail-notifications).

## Dashboard files (`dashboards/`)

```json
{
  "name": "Production",
  "priority": 10,
  "hosts": { "groups": ["production"] },
  "panels": [
    { "type": "problems", "x": 0, "y": 0, "w": 8, "h": 6 },
    { "type": "host-states", "x": 8, "y": 0, "w": 4, "h": 2 },
    { "type": "value", "host": "web-01", "item": "cpu.usage", "x": 8, "y": 2, "w": 4, "h": 1 },
    {
      "type": "graph",
      "host": "web-01",
      "item": "disk.used_percent",
      "params": { "mount": "/var" },
      "range": "6h",
      "x": 8, "y": 3, "w": 4, "h": 3
    }
  ]
}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `name` | yes | Display name. Unique. |
| `priority` | no | Whole number, default 0. |
| `hosts` | no | Selects the hosts of the dashboard: `groups` (a host must be in all of them) and `names` (single hosts, added to the group match). With both empty or absent, every host. |
| `panels` | no | The panels. When absent, the standard layout is used: a problems panel (8 columns wide) next to a host-states panel (4 columns wide), full height. |

Panels are placed on a grid of 12 columns and 6 rows. `x` and `y` are the top left cell
(starting at 0), `w` and `h` the size in cells. A panel must lie inside the grid and panels
must not overlap, so a dashboard has at most 72 panels. Panel types:

| `type` | Extra fields | Shows |
| --- | --- | --- |
| `problems` | none | Open problems. |
| `host-states` | none | Number of hosts per state. |
| `graph` | `host`, `item`, `params`, `range` (default `6h`) | A time series. |
| `value` | `host`, `item`, `params` | The latest value. |

For `graph` and `value`, the host must exist, be selected by the dashboard's `hosts`, and
collect the item. An item that takes a parameter (`disk.*` items take `mount`, `net.*` items
take `interface`) needs exactly that parameter in `params`: a panel shows one series, and
without the parameter it would match every mount or interface.

A dashboard that only selects hosts and uses the standard layout. Save it as
`dashboards/web.json`: its id is then `web`, which the display example below refers to.

```json
{
  "name": "Web servers",
  "hosts": { "groups": ["web"] }
}
```

## Display files (`displays/`)

A display is a screen, for example a monitor in an operations room.

```json
{
  "name": "NOC",
  "show": { "rotate": ["production", "web"], "every": "30s" },
  "switch_on_alarm": { "min_severity": "critical" }
}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `name` | yes | Display name. Unique. |
| `show` | yes | Either `{ "dashboard": "<id>" }` to show one dashboard, or `{ "rotate": ["<id>", ...], "every": "30s" }` to cycle through dashboards. `every` must be between `5s` and `1h`. |
| `switch_on_alarm` | no | Switch to a dashboard with a problem of at least `min_severity` (default `critical`). |
| `dashboards` | no | A list of dashboard ids. Every dashboard in `show` must be in this list, and each id must exist. |

Dashboards are referenced by id (file name without `.json`), and each must exist.
A fixed display:

```json
{
  "name": "Lobby",
  "show": { "dashboard": "production" }
}
```

On the monitor, `every` is how long each dashboard of the rotation stays on the screen;
`switch_on_alarm.min_severity` is the lowest severity of an unacknowledged problem that makes the
display switch to the dashboard showing it, with a band that says "Alarm: showing <dashboard>.";
and `dashboards` limits the dashboards it may switch to (without it, any dashboard). How to set
up the monitor, its token and its browser: [Wall displays](web-ui.md#wall-displays).

## Names

Group names, template names and the ids of dashboards and displays use 1 to 63 lowercase letters,
digits, `_` or `-`, starting with a letter or digit. Host names follow the rule given above.

## Trigger expressions

An expression is a condition on one item, or a check that data stopped arriving.

A trigger opens a problem and sends an e-mail when the condition holds; see [Alerting](alerting.md) for how holding, instances and no-data detection work.

```
expression  = comparison | no-data
comparison  = selector operator number [ "for" duration ]
no-data     = [ selector ] "no" "data" "for" duration
selector    = item-key [ "{" param "=" value { "," param "=" value } "}" ]
operator    = ">" | "<" | ">=" | "<=" | "==" | "!="
```

- `item-key` is a [built-in item](items.md). `param` is `mount` or `interface`. A `value` is
  written bare (`/var`, `eth0`; it ends at `,`, `}`, a space or a quote) or in double quotes
  when it contains such characters (`{mount="/mnt/my disk"}`). A quoted value cannot contain a
  double quote itself; there is no escape for it.
- `number` is a decimal number, optionally negative, such as `90`, `0.5` or `-1`.
- `duration` uses the interval form (`10m`, `1h`).
- `for duration` means the condition must hold for that long before the problem opens. Without
  it, the condition applies immediately.
- `no data for 3m` applies to the host as a whole. With a selector, to that item only.
- White space between the parts is free.

Examples:

| Expression | Meaning |
| --- | --- |
| `cpu.usage > 90` | CPU usage above 90 %. |
| `disk.used_percent{mount=/} >= 95 for 10m` | Root file system at least 95 % full for 10 minutes. |
| `net.rx_bytes_per_sec{interface=eth0} == 0 for 5m` | No traffic received on `eth0` for 5 minutes. |
| `no data for 3m` | The host sent nothing for 3 minutes. |
| `disk.used_percent{mount=/} no data for 1h` | No value for that item for an hour. |

A syntax error is reported with its column. A trigger can only watch an item that its host
collects; this is checked per host, including for triggers that come from a template.

## Validation

The server checks the whole repository before it changes anything. A revision is applied only if
every check passes; otherwise the previous configuration stays active ("all or nothing"). The
checks are:

- Every file is valid UTF-8 and valid JSON, matches its file type, and has no unknown fields.
- Names are unique within a kind (host names, template names, notification names, dashboard and
  display ids and names), and follow the [name rules](#names).
- Item keys are built-in items; parameters are the ones the item takes.
- Intervals are valid; trigger expressions parse; severities are one of the four values.
- Every template a host lists exists and is listed once; a host lists at most 32 templates.
- No key or string value contains a control character.
- Every trigger watches an item its host collects.
- A host with an item-level no-data trigger (`cpu.usage no data for 5m`) also has a host-level one
  (`no data for 3m`). An item-level trigger fires only for an item that stopped while the rest
  of the host went on reporting; when the whole host is silent only the host-level trigger
  fires, at its own window, so that an agent outage is one problem. Without it the outage would
  give no problem at all. After a server start, no-data detection begins counting from that
  start, so a restart never makes hosts look silent.
- Notifications have at least one valid address.
- Dashboards: at most 72 panels, panels inside the grid, no overlap (the first overlapping pair
  is reported), graph and value panels refer to existing hosts
  that the dashboard selects and that collect the item, and give the parameter of an item that
  takes one (`mount` or `interface`).
- Displays: every referenced dashboard exists; rotation has at least one dashboard and an `every`
  from 5 s to 1 h.
- The [limits](#layout) on file size, file count and total size.

At most 50 problems are reported per revision. Each problem names the file and, for JSON
syntax errors, the line and column.

## Out Of Sync

The installation is **Out Of Sync** when the latest sync attempt did not produce a valid
configuration: the repository could not be fetched (unreachable, credentials rejected, branch
missing, `git` timed out after 60 s), the revision is invalid, it was refused by the
[mass-removal guard](#mass-removal-guard), or the database could not store it (shown as
`the configuration could not be stored: ...`; the server log has the full error). While Out Of Sync, the last valid configuration stays
active: hosts keep being monitored and agents keep their configuration. The state clears by
itself with the next successful sync, for example after you push a fix.

Read it with `admin sync status`:

```
out of sync
active commit: ff92a585d63b9b3491fd8ee9c28f0d087ceaa5f6
last successful sync: 2026-11-15T00:57:34.462789Z
last attempt: 2026-11-15T00:57:39.465508Z
hosts/broken.json:1:20: invalid JSON: EOF while parsing a value
```

The first line is `in sync`, `out of sync` or `never synced`. `active commit` is the revision
that is in use. Each following line is one problem, as `file:line:column: reason`; failures
without a file (such as an unreachable repository) are shown as the reason alone. `admin sync
history` lists the applied revisions:

```
2026-11-15T00:59:03.819986Z d505d3987c78 hosts +0 ~0 -0
2026-11-15T00:58:03.422969Z d05a1cbfde7e hosts +3 ~0 -0, templates +1, notifications +1, dashboards +1, displays +1
```

`+` is added, `~` changed and `-` removed.

## Mass-removal guard

A host that has no file in the repository is deactivated: its configuration is dropped and its
agent credential is deleted. To protect against an accidental deletion of a directory, a revision
that would deactivate too many hosts at once is refused and the installation goes Out Of Sync
with a reason such as:

```
this revision removes 10 of 12 active hosts; raise gitops.max_host_removal_percent to apply it
```

The limit is `CERBRIX_GITOPS__MAX_HOST_REMOVAL_PERCENT` (default 50). The guard only applies when
at least 10 hosts are active, and a revision is refused when the removed hosts are more than the
given percentage of the active hosts. If the removal is intended, raise the value, wait for the
sync, and lower it again.

A host that reappears in the repository becomes active again. Because its credential was
deleted, its agent enrols again (see [install.md](install.md)).

## Checking a change before you push it

`cerbrix-server check-config <dir>` runs the same checks on a local checkout. It needs no
configuration file and no database. It prints a summary and exits with 0, or prints one line per
problem and exits with 1:

```
$ cerbrix-server check-config .
configuration is valid: 1 host, 1 template, 1 notification, 2 dashboards, 2 displays

$ cerbrix-server check-config .
hosts/web-02.json: unknown item key "cpu.usge": the built-in items are listed in docs/items.md
hosts/web-02.json: unknown template "nope": there is no file in templates/ that defines it
```

With the image: `docker run --rm -v "$PWD:/config:ro" cerbrix-server:local check-config /config`.

### In your own CI

To stop a broken change from reaching the server, run the check on every pull request. The
image reference below is a placeholder, because no Cerbrix image is published yet: build
`cerbrix-server` from this repository, push it to a registry you control and use that reference.

```yaml
name: check-config
on:
  pull_request:
  push:
    branches: [main]

jobs:
  check-config:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Validate the configuration
        run: >
          docker run --rm -v "$PWD:/config:ro"
          registry.example.com/cerbrix-server:latest check-config /config
```

The step fails when `check-config` reports a problem.

## Schemas in the editor

The directory [`schemas/`](../schemas/) has a JSON Schema for each file type: `host`,
`template`, `notification`, `dashboard` and `display`. They give completion and error
highlighting while you edit. The schemas are generated from the same types the server uses.
They check the structure of a file; rules that involve several files (a template exists, an item
is collected by the host) are only checked by `check-config` and the server.

Files in the repository must not contain a `$schema` key, because the server rejects unknown
fields. Map the schemas to the directories in your editor instead. In VS Code, add this to the
settings (`.vscode/settings.json` of your configuration repository):

```json
{
  "json.schemas": [
    {
      "fileMatch": ["/hosts/**/*.json"],
      "url": "https://raw.githubusercontent.com/TrisMarzec/Cerbrix/main/schemas/host.schema.json"
    },
    {
      "fileMatch": ["/templates/**/*.json"],
      "url": "https://raw.githubusercontent.com/TrisMarzec/Cerbrix/main/schemas/template.schema.json"
    },
    {
      "fileMatch": ["/notifications/**/*.json"],
      "url": "https://raw.githubusercontent.com/TrisMarzec/Cerbrix/main/schemas/notification.schema.json"
    },
    {
      "fileMatch": ["/dashboards/**/*.json"],
      "url": "https://raw.githubusercontent.com/TrisMarzec/Cerbrix/main/schemas/dashboard.schema.json"
    },
    {
      "fileMatch": ["/displays/**/*.json"],
      "url": "https://raw.githubusercontent.com/TrisMarzec/Cerbrix/main/schemas/display.schema.json"
    }
  ]
}
```

A local copy of the schema files works too: use a relative path such as
`"url": "./schemas/host.schema.json"`.

## What the agents do with it

An agent fetches its configuration (the list of items and intervals) from the server when it
starts and then every 60 seconds, and applies changes without a restart. It keeps the last
configuration it received in its state directory and keeps measuring with it when the server is
unreachable, also across a restart. Until it has received a configuration it measures nothing.
The `[intervals]` section of `agent.toml` no longer exists: the agent refuses to start with it,
so remove it when you upgrade an existing agent.
