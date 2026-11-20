# The Cerbrix API

The server has a JSON API under `/api/v1`. The web UI, which the server serves at `/` (see [The web UI](web-ui.md)), uses it, and so can scripts. This
page describes every route. For how to set up logins see [Installing the server](install.md#3-first-login).

The examples were taken from a real run of the end-to-end check (`scripts/e2e.sh`) against a
server whose configuration comes from Git. Long lists are shortened to one or two entries,
tokens, passwords and keys are replaced by placeholders such as `<token>`, and the
synchronisation fields of the status and display examples show a healthy installation (the check
had already deleted its repository when they were taken).

- [Conventions](#conventions)
- [Login and sessions](#login-and-sessions)
- [Roles](#roles)
- [Display tokens](#display-tokens)
- [Routes](#routes): [session](#session), [hosts](#hosts), [problems](#problems),
  [dashboards](#dashboards), [displays](#displays), [status and configuration](#status-and-configuration),
  [users](#users)
- [What is not there](#what-is-not-there)

## Conventions

**Base path and format.** All routes start with `/api/v1`. Requests with a body send JSON with
`Content-Type: application/json`, at most 64 KiB; answers are JSON. A request with a body and
another content type is answered `400`, a body over the limit `413`. The body of a `POST`, `PUT` or `PATCH` must be a JSON object, and it must be there even when
every field is optional: `POST /problems/{id}/ack`, `POST /problems/{id}/close`,
`POST /users/{id}/password` and `PATCH /users/{id}` need at least `{}`; a request without a body, or
without the content type, is `400 invalid`. (`POST /displays/{id}/token`, `POST /hosts/{name}/release`
and the `DELETE` routes take no body.) Bodies with an unknown field are refused with `400 invalid`
on `ack`, `close`, `POST /enrollment-keys`, `POST /users`, `PATCH /users/{id}` and
`POST /users/{id}/password`; the login and the password change ignore unknown fields. Every answer carries `Cache-Control: no-store`
and `X-Content-Type-Options: nosniff`.
A path under `/api/v1` that is not listed below is `404`, in the same error format. A method that
is not listed for a path that is (for example `PUT /api/v1/hosts`) is `405 Method Not Allowed`
with an empty body.

**Times.** Times in answers are RFC 3339 in UTC, for example `2026-11-16T21:54:05.114Z`. The
points of a series are the exception: each is `[timestamp, value]` with the timestamp in
milliseconds since 1970.

**Names and text.** Text that comes from the configuration or from users (host names, trigger
names, summaries, comments, dashboard names) is returned as JSON strings, never as HTML.
Display it as text.

**Errors.** An error has the HTTP status and a body with a stable `error` code and a `message`
for people. Test the code, not the message.

```json
{"error":"invalid","message":"limit must be between 1 and 500"}
```

| Code | Status | Meaning |
| --- | --- | --- |
| `invalid` | 400, 413 | A parameter or the body is wrong; the message names the field. Also a body that is not JSON, a body over 64 KiB (413), and a free-text query parameter with more characters than allowed or with a control character, NUL included. |
| `unauthorized` | 401 | No valid session or token, or a failed login. |
| `forbidden` | 403 | The role is too low, the request comes from another origin, a display may not show a dashboard, or the current password given on a password change is wrong. |
| `not_found` | 404 | The thing does not exist, or its identifier cannot be one. Also an unknown path. |
| `conflict` | 409 | The change clashes with the state: a user name is taken, a problem is already resolved, or the last enabled admin would be lost. |
| `too_many_attempts` | 429 | The server is busy checking passwords: too many logins, password changes or user creations at once. For a login it also means that the client address had too many failed logins. Nothing was checked or changed, and it does not count as a failed login. Try again shortly. See *Busy server* and *Failed logins* under [Login and sessions](#login-and-sessions). |
| `metrics_unavailable` | 503 | The metrics store did not answer. Only the [series of a host](#get-apiv1hostsnameseries-viewer) uses it. |
| `database_unavailable` | 503 | PostgreSQL cannot be reached. Any route can answer it, the login and the check of a session or display token included. See *Database down* below. |
| `timeout` | 503 | The request was not answered within 25 seconds. Nothing tells whether a change it asked for was made; read the state again before repeating it. |
| `internal` | 500 | A fault of the server; the cause is in the server log, not in the answer. |

**Lists.** A list that can be long takes `limit` (1 to 500, default 50) and `offset` (from 0,
default 0) and answers `{"items": [...], "total": N}`, where `total` counts all matching items.
It is said for each route whether it pages. The lists that do not page are short by nature.

**Current values lag.** VictoriaMetrics, where the values are stored, hides the newest 30
seconds of data from queries (its `-search.latencyOffset`). The current values the API shows
(`cpu` and `memory` of the hosts list, the values of a host, `value` panels, and the newest point
of a series) can therefore be about 30 seconds older than what the agent has just sent. Problems
and their states do not lag: the server evaluates triggers when the values arrive.

**Metrics store down.** The hosts list, the values of a host and the dashboards do not fail when
the metrics store cannot be read. The data that could not be read is `null` (or an empty list),
and `metrics_available` is `false`. The rest of the answer, which comes from PostgreSQL, is
complete.

**Database down.** When PostgreSQL cannot be reached, a request is answered within about 5
seconds (the time the server waits for a database connection) with `503 database_unavailable`,
in the error format and with the usual headers. A session is not ended by it: the check of the
session itself answers 503, not 401. The web UI shows that the server cannot be reached and
asks again by itself until the database is back.

## Login and sessions

A user logs in with a name and a password and gets a session. The session is a cookie:

```
cerbrix_session=<token>; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800
```

The cookie has `Secure` when the setting `auth.secure_cookie` says so, see
[Login settings](install.md#login-settings). The token is random, only its hash is stored in
PostgreSQL, so sessions survive a restart of the server. A session ends when:

- the user logs out (`DELETE /api/v1/session`),
- it was not used for `auth.session_idle_secs` (default 12 hours),
- `auth.session_max_secs` have passed since the login (default 7 days),
- the user is disabled or deleted, or an admin changes the user's role or resets the password,
- the user changes the own password: all sessions of the user end, the current one included; the
  answer to the change sets the cookie of a new session (with the limits of a fresh login), so the
  browser that made the change stays logged in and a copy of the old cookie does not.

A request with an unknown or ended session is answered `401` and removes the cookie.

**Failed logins.** A wrong password, an unknown name, a disabled user and a locked user all get
the same `401` with the same message. After five wrong passwords in a row a user is locked for
five minutes; the lock ends by itself, a right password after that resets the count, and so does
a password reset by an admin. A wrong current password on a password change counts like a wrong
password at the login.

Failed logins are also counted per client address: after 10 failed logins from one address
within 5 minutes (an IPv6 address counts as its /64), the login answers
`429 too_many_attempts` to that address until the 5 minutes from its first failure are over,
with the message of a busy server, whatever the name and password. Nothing is checked then,
and it does not count against the user. A successful login and a busy server do not count.
Which address counts behind a reverse proxy is a setting, see
[Login settings](install.md#login-settings).

**Busy server.** Checking and storing a password (Argon2id) takes a lot of memory and CPU, so the
server runs only a few of these at once (one per processor core, at least 2 and at most 8). A
request that has to check or store a password waits for its turn at most 5 seconds; then it is
answered `429 too_many_attempts` with the message
`the server is busy checking passwords, try again shortly`. The answer is the same whatever the
user name, an existing, unknown or locked one; nothing was checked, so it does not count as a
failed login. This concerns the login, the password change, creating a user and resetting a
password.

**Same-origin rule.** Requests that change something (every method except `GET` and `HEAD`) are
checked against cross-site requests. If the request has an `Origin` header, its host and port must
be those of the `Host` header; the scheme is not compared, so a proxy that terminates TLS does not
matter, but it must pass `Host` on unchanged. Without `Origin`, a `Sec-Fetch-Site` header must say
`same-origin`. A request with neither header does not come from a browser page and is let
through. Anything else is `403 forbidden`.

**Logging in without a browser.** `curl` keeps the cookie in a file. Use `--data-binary @file`
for the password so that it does not appear in the process list:

```sh
printf '{"name":"t.marzec","password":"%s"}' "$PASSWORD" > login.json
curl -sS -c cookies.txt -H 'Content-Type: application/json' \
     --data-binary @login.json https://cerbrix.example.com/api/v1/session
curl -sS -b cookies.txt https://cerbrix.example.com/api/v1/status
curl -sS -b cookies.txt -X POST https://cerbrix.example.com/api/v1/problems/<id>/ack \
     -H 'Content-Type: application/json' -d '{"comment":"looking into it"}'
rm login.json
```

The first user is created on the server with `cerbrix-server admin user create`, see
[First login](install.md#3-first-login).

## Roles

Every user has one role. Each role may do what the roles before it may do.

| Role | May |
| --- | --- |
| `viewer` | Read everything: hosts, problems, dashboards, displays, status, sync history. Change the own password. |
| `operator` | Also acknowledge and close problems, and read the unknown agents. |
| `admin` | Also manage users, enrolment keys and display tokens, and release the agent binding of a host. |

Each route below states the access it needs in its heading: `public` (the login only), `viewer`,
`operator`, `admin`, or `display` (a logged-in user of any role, or the token of that display, see
the next section). A session that does not have the role gets `403`, a request without a valid
session `401`.

With GitOps on, hosts, templates, notifications, dashboards and displays are configuration and
come from the Git repository; the API only reads them. The things the API changes (users,
sessions, problems, enrolment keys, display tokens and agent bindings) are not configuration
and can be changed with GitOps on.

## Display tokens

A wall display (a monitor in an operations room) has no user. It uses a token that an admin
creates for that one display.

- The token is accepted only on the two routes `GET /api/v1/display/{id}` and
  `GET /api/v1/display/{id}/dashboards/{dashboard}`, and only for the display whose `{id}` it
  was created for. Everywhere else, and for any other display, it is `401`, the same as for an
  unknown token. Send it as `Authorization: Bearer <token>`. A request that carries both a valid
  session cookie and a token is treated as the user.
- The token is shown once, in the answer of `POST /api/v1/displays/{id}/token`. Only its hash is
  stored. If it is lost, create a new one.
- Creating a token replaces the old one, which stops working at once. An admin can also revoke
  it with `DELETE /api/v1/displays/{id}/token`. A display has at most one token.
- What a token can read: the display's setting and alarm target, the dashboards it may show,
  and the sync state of the configuration as `/api/v1/status` shows it. That includes the issues
  of a failed synchronisation, with the file names, positions and reasons, because the wall shows
  "out of sync" with them. Whoever holds the token of a display can read these.
- What a token cannot read: who acknowledged or closed a problem, and their comments. In the
  answer of the dashboard route for a token, `acknowledged_by`, `ack_comment`, `closed_by` and
  `close_comment` of every problem are `null`; `acknowledged_at` is there, so the wall still
  shows a problem as acknowledged. A logged-in user gets the full answer on the same route.
- When a display leaves the configuration (its file is removed from the repository and the
  change is applied), its token is removed with it.
- A token can only be created for a display that is in the active configuration. If an
  admin creates a token at the very moment a synchronisation with the repository is applying a
  change, the answer can be `404` once. Repeat the request.
- Polling. `GET /api/v1/display/{id}` costs a fixed, small number of database queries, however
  many dashboards and hosts exist. A monitor can poll it every 5 to 10 seconds. The dashboard
  route costs a handful of queries and one call to the metrics store for each graph or value panel.

## Routes

Each route has a heading of a fixed form: the method and the path in backticks, then the access in
parentheses. The test `every_route_is_documented` reads these headings and fails when a route has
no heading or a heading names a wrong access.

### Session

### `POST /api/v1/session` (public)

Logs in. Body: `name` and `password`, both strings. The answer is the user, and the session
cookie is set.

```sh
curl -sS -i -c cookies.txt -H 'Content-Type: application/json' \
     --data-binary @login.json http://127.0.0.1:18443/api/v1/session
```

```
HTTP/1.1 200 OK
content-type: application/json
set-cookie: cerbrix_session=<token>; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800
cache-control: no-store
x-content-type-options: nosniff

{"name":"docs-admin","role":"admin"}
```

A wrong password, an unknown name, a disabled user and a locked user are all
`401 unauthorized` with the message `wrong user name or password, or the account is temporarily locked`.
A server that is too busy checking passwords answers `429 too_many_attempts`, the same for every
name, see *Busy server* under [Login and sessions](#login-and-sessions). So does a login from a
client address with 10 failed logins in the last 5 minutes, see *Failed logins* there.

### `GET /api/v1/session` (viewer)

The logged-in user.

```json
{"name":"docs-admin","role":"admin"}
```

### `DELETE /api/v1/session` (viewer)

Logs out: ends this session and removes the cookie. Answer `204`. Other sessions of the user
stay.

### `PUT /api/v1/session/password` (viewer)

Changes the own password. Body: `current` and `new`. The new password has 12 to 256 characters and
no NUL; one that does not is `400 invalid`, checked before the current password and without
counting as a failed attempt. On success the answer is `204`, every session of the user ends, the
current one included, and the answer sets the cookie of a new session (`Set-Cookie`, as at the
login): keep using the new cookie, the old one is `401`. A wrong `current`
is `403 forbidden` and counts as a failed attempt for the lock; a locked user gets the same
answer. When the password was changed in the meantime, for example reset by an admin while
the request was being processed, the answer is `409 conflict` ("the password was changed in the
meantime") and nothing changes: the password and the sessions stay as that other change left
them. A busy server answers `429 too_many_attempts` and changes nothing, see
*Busy server* under [Login and sessions](#login-and-sessions).

```sh
curl -sS -b cookies.txt -X PUT -H 'Content-Type: application/json' \
     -d '{"current":"<current>","new":"<new password>"}' http://127.0.0.1:18443/api/v1/session/password
```

```json
{"error":"forbidden","message":"the current password is wrong"}
```

### Hosts

### `GET /api/v1/hosts` (viewer)

The active hosts, paged (`limit`, `offset`). Hosts with an unacknowledged problem come first, the most severe first, then hosts whose problems are all acknowledged, then the others; within each group they are sorted by name. Parameters:

| Parameter | Meaning |
| --- | --- |
| `search` | Text that the name or the address contains, upper and lower case alike, taken literally. Up to 200 characters. |
| `group` | Only hosts in this group (exact name). Up to 63 characters. |
| `state` | `problems` (at least one open problem), `no_data` (at least one open no-data problem) or `ok` (no open problem). |

`state` of a host is `null` without open problems; otherwise it is the state of the problem that
decides: unacknowledged before acknowledged, then the higher severity, then the older problem.
`acknowledged` is true when every open problem of the host is acknowledged. `cpu` and `memory`
are the newest `cpu.usage` and `memory.used_percent` of the last ten minutes, or `null`.
`metrics_available` is `false` when they could not be read. The values lag by about 30 seconds,
see [Conventions](#conventions). `managed_by_git` is true when GitOps is on.

```sh
curl -sS -b cookies.txt 'http://127.0.0.1:18443/api/v1/hosts?search=e2e&state=problems'
```

```json
{"items":[{"name":"e2e-host","address":null,"groups":["e2e"],"state":{"severity":"critical","acknowledged":false,"kind":"no_data","trigger":"Agent silent","instance":"","summary":"no data for 1m","open_problems":2},"last_seen_at":"2026-11-16T21:53:02.051491Z","managed_by_git":true,"cpu":29.407859802246,"memory":46.610436858251}],"total":1,"metrics_available":true}
```

### `GET /api/v1/host-groups` (viewer)

All group names, sorted. Not paged.

```json
{"items":["db","e2e","production","web"]}
```

### `GET /api/v1/hosts/{name}` (viewer)

One host with its effective configuration (what the host and its templates give together) and
its open problems. `origin` of an item or trigger is `host` or `template:<name>`; `firing` says
whether the trigger has an open problem. A host created by hand has no configuration: `templates`,
`items` and `triggers` are empty. `agent_version` is the version the agent reported. It is known from enrolment on (the
`agent_version` of the enrolment request, stored when it is a valid version) and follows the
`User-Agent` of the agent's requests with values (`cerbrix-agent/<version>`). It is `null` after
the host's agent binding was released (`POST /api/v1/hosts/{name}/release` or
`admin host release`) until the agent enrols again, and `null` for a host whose agent has not
enrolled yet. `dashboards` lists the dashboards that show the host (`id` and
`name`), sorted by name; it is empty for a host in none. `404` for an unknown host.

```json
{"name":"e2e-host","address":null,"groups":["e2e"],"templates":["linux-base"],"source_file":"hosts/e2e-host.json","managed_by_git":true,"enrolled_at":"2026-11-16T21:52:06.337437Z","last_seen_at":"2026-11-16T21:53:02.051491Z","agent_version":"0.1.0","dashboards":[{"id":"e2e","name":"E2E"}],"state":{"severity":"critical","acknowledged":false,"kind":"no_data","trigger":"Agent silent","instance":"","summary":"no data for 1m","open_problems":2},"items":[{"key":"cpu.usage","params":null,"interval_secs":5,"origin":"host"}],"triggers":[{"name":"Agent silent","expression":"no data for 1m","severity":"critical","origin":"host","firing":true},{"name":"Disk almost full","expression":"disk.used_percent > 90 for 10m","severity":"high","origin":"template:linux-base","firing":false}],"open_problems":[{"id":"0c208a84-6643-46d4-86ab-066f04916ecc","host":"e2e-host","trigger":"Agent silent","instance":"","severity":"critical","kind":"no_data","summary":"no data for 1m","started_at":"2026-11-16T21:54:05.114Z","resolved_at":null,"resolve_reason":null,"acknowledged_at":null,"acknowledged_by":null,"ack_comment":null,"closed_by":null,"close_comment":null}]}
```

(`items`, `triggers` and `open_problems` are shortened here.)

### `GET /api/v1/hosts/{name}/values` (viewer)

The newest value of every series of the host that has one from the last hour: one entry per item
and, for items with a parameter such as `disk.used_percent`, per parameter value. Sorted by key.
Not paged. The values lag by about 30 seconds, see [Conventions](#conventions). When the metrics
store cannot be read, `items` is empty and `metrics_available` is `false`. `404` for an unknown
host.

```json
{"items":[{"key":"cpu.usage","params":{},"value":29.407859802246,"at":"2026-11-16T21:53:02.609Z"},{"key":"disk.used_percent","params":{"mount":"C:\\"},"value":90.3023661557,"at":"2026-11-16T21:53:02.609Z"}],"metrics_available":true}
```

### `GET /api/v1/hosts/{name}/series` (viewer)

The history of one series, for a graph. Parameters, each given once:

| Parameter | Meaning |
| --- | --- |
| `item` | The item key, for example `cpu.usage`. It must be an item of the host. Required. |
| `range` | `1h` (points 15 s apart), `6h` (90 s), `24h` (5 min) or `7d` (30 min). Required. |
| `p_<name>` | The parameter of the item, for example `p_mount=/` for `disk.used_percent` or `p_interface=eth0` for `net.*`. 1 to 256 characters. |

An item with a parameter needs it: without `p_mount` a `disk.used_percent` request would match
the points of several disks, so it is refused with `400 invalid`. An item without a parameter takes
none. An unknown host is `404`; an unknown query parameter, or one given twice, is `400 invalid`.
`thresholds` are the limits of the host's comparison triggers on that item, to draw into the
graph. The newest point can be about 30 seconds old, see [Conventions](#conventions). If the
metrics store cannot be read the answer is `503 metrics_unavailable`.

```sh
curl -sS -b cookies.txt 'http://127.0.0.1:18443/api/v1/hosts/e2e-host/series?item=cpu.usage&range=1h'
```

```json
{"key":"cpu.usage","params":{},"from":"2026-11-16T20:54:59.336199379Z","to":"2026-11-16T21:54:59.336199379Z","step_secs":15,"points":[[1794865935000,28.2028656006],[1794865950000,27.432800292968],[1794865965000,24.736618042]],"thresholds":[{"trigger":"E2E always","severity":"high","op":"<","value":0.0}]}
```

(`points` is shortened here.) Without the parameter:

```json
{"error":"invalid","message":"disk.used_percent needs the parameter p_mount"}
```

### Problems

A problem has these fields; every route that returns a problem uses this form.

| Field | Meaning |
| --- | --- |
| `id` | The id of the problem. |
| `host`, `trigger`, `instance` | What it is about. `instance` is empty, or the parameters of the series, such as `mount=/`. |
| `severity` | `info`, `warning`, `high` or `critical`. |
| `kind` | `compare` (a comparison trigger) or `no_data`. |
| `summary` | A short text such as `memory.used_percent >= 0: value 47.123`. |
| `started_at`, `resolved_at` | When it opened and ended; `resolved_at` is `null` while it is open. |
| `resolve_reason` | Why it ended: `condition_ended`, `host_removed`, `trigger_removed` or `closed_by_operator`. |
| `acknowledged_at`, `acknowledged_by`, `ack_comment` | The acknowledgement, `null` when there is none. |
| `closed_by`, `close_comment` | Who closed the problem by hand, and why. Only set by closing. |

### `GET /api/v1/problems` (viewer)

Problems, paged (`limit`, `offset`). Open problems are listed unacknowledged first, then by severity (most severe first), then newest first (the start time, then the id); resolved problems and `all` newest first. The answer also has `dashboards`: for every host that appears in `items`, the dashboards that show it (`id` and `name`, sorted by name), an empty list for a host in none. Parameters:

| Parameter | Meaning |
| --- | --- |
| `state` | `open` (default), `resolved` or `all`. |
| `severity` | The lowest severity: `info`, `warning`, `high` or `critical`. Problems of that severity and above are listed. |
| `host` | Exact host name. A name that cannot be a host name gives an empty list. |
| `group` | Hosts in this group. Up to 63 characters. |
| `dashboard` | The id of a dashboard (its file name without `.json`): only problems of the hosts it shows. An unknown id is `400 invalid`. Up to 200 characters. |
| `search` | Text that the host name, trigger name or summary contains, taken literally. Up to 200 characters. |

```sh
curl -sS -b cookies.txt 'http://127.0.0.1:18443/api/v1/problems?state=open&limit=2'
```

```json
{"items":[{"id":"0c208a84-6643-46d4-86ab-066f04916ecc","host":"e2e-host","trigger":"Agent silent","instance":"","severity":"critical","kind":"no_data","summary":"no data for 1m","started_at":"2026-11-16T21:54:05.114Z","resolved_at":null,"resolve_reason":null,"acknowledged_at":null,"acknowledged_by":null,"ack_comment":null,"closed_by":null,"close_comment":null},{"id":"6fa4ff3a-6224-462b-a96f-ab6f3daca0f1","host":"e2e-host","trigger":"E2E second","instance":"","severity":"warning","kind":"compare","summary":"memory.used_percent >= 0: value 47.123","started_at":"2026-11-16T21:52:07.614Z","resolved_at":null,"resolve_reason":null,"acknowledged_at":"2026-11-16T21:54:28.337312Z","acknowledged_by":"e2e-admin","ack_comment":"e2e api check","closed_by":null,"close_comment":null}],"total":2,"dashboards":{"e2e-host":[{"id":"e2e","name":"E2E"}]}}
```

### `GET /api/v1/problems/{id}` (viewer)

One problem, in the form above. `404` for an unknown id.

### `POST /api/v1/problems/{id}/ack` (operator)

Acknowledges an open problem: it records who looked at it and why, and it stops counting as an
alarm on dashboards and wall displays. The problem stays open. Body: `comment`, optional, text
that is trimmed and has its control characters replaced by spaces; it is cut after 1,000
characters. Acknowledging again replaces the earlier acknowledgement. `409 conflict` when the
problem is already resolved, `404` for an unknown id. The name is the logged-in user's.

```sh
curl -sS -b cookies.txt -X POST -H 'Content-Type: application/json' \
     -d '{"comment":"disk replaced, watching"}' \
     http://127.0.0.1:18443/api/v1/problems/6fa4ff3a-6224-462b-a96f-ab6f3daca0f1/ack
```

```json
{"id":"6fa4ff3a-6224-462b-a96f-ab6f3daca0f1","host":"e2e-host","trigger":"E2E second","instance":"","severity":"warning","kind":"compare","summary":"memory.used_percent >= 0: value 47.123","started_at":"2026-11-16T21:52:07.614Z","resolved_at":null,"resolve_reason":null,"acknowledged_at":"2026-11-16T21:55:20.993874Z","acknowledged_by":"docs-admin","ack_comment":"disk replaced, watching","closed_by":null,"close_comment":null}
```

### `POST /api/v1/problems/{id}/close` (operator)

Closes an open problem by hand, for an instance that no longer exists, such as an unmounted disk
or a removed network interface. It is not the same as acknowledging: the problem ends
(`resolved_at` is set, `resolve_reason` is `closed_by_operator`), and `closed_by` and
`close_comment` record who closed it and why. An acknowledgement stays as it was. The trigger
state of that instance is dropped; if the condition still holds, the next value opens a new
problem. With e-mail on, those who were mailed the start get the end message.

Body: `comment`, required. A comment that is empty after trimming is `400 invalid`. `409 conflict`
when the problem is already resolved, `404` for an unknown id.

```sh
curl -sS -b cookies.txt -X POST -H 'Content-Type: application/json' \
     -d '{"comment":"instance removed"}' \
     http://127.0.0.1:18443/api/v1/problems/6fa4ff3a-6224-462b-a96f-ab6f3daca0f1/close
```

```json
{"id":"6fa4ff3a-6224-462b-a96f-ab6f3daca0f1","host":"e2e-host","trigger":"E2E second","instance":"","severity":"warning","kind":"compare","summary":"memory.used_percent >= 0: value 47.123","started_at":"2026-11-16T21:52:07.614Z","resolved_at":"2026-11-16T21:55:21.351491Z","resolve_reason":"closed_by_operator","acknowledged_at":"2026-11-16T21:55:20.993874Z","acknowledged_by":"docs-admin","ack_comment":"disk replaced, watching","closed_by":"docs-admin","close_comment":"instance removed"}
```

### Dashboards

### `GET /api/v1/dashboards` (viewer)

All dashboards with their state, for a start page. Not paged. `id` is the file name without
`.json`. A host counts once, in the group of its state: `critical`, `high`, `warning`, `info` (the
severity of the problem that decides, unacknowledged), `acknowledged` (all its problems are
acknowledged) or `ok`. `open_problems` and `unacknowledged` count problems per severity.

```json
{"items":[{"id":"e2e","name":"E2E","file":"dashboards/e2e.json","priority":0,"host_count":1,"hosts_by_state":{"critical":1,"high":0,"warning":0,"info":0,"acknowledged":0,"ok":0},"open_problems":{"critical":1,"high":0,"warning":1,"info":0},"unacknowledged":{"critical":1,"high":0,"warning":0,"info":0}}]}
```

### `GET /api/v1/dashboards/{id}` (viewer)

Everything one dashboard shows: its hosts with their state and its panels, each with its place on
the 12 by 6 grid (`x`, `y`, `w`, `h`) and its data. `404` for an unknown dashboard.

| Panel `type` | Data |
| --- | --- |
| `problems` | `problems`: the open problems of the dashboard's hosts (at most 100; unacknowledged first, most severe first, newest first), and `more`, how many open problems are not in the list. |
| `host-states` | `total`, `without_problems` and `hosts`, the hosts with their state, problem hosts first. |
| `graph` | `host`, `item`, `params`, `range`, `step_secs`, `points` (`[timestamp_ms, value]`) and `thresholds`. |
| `value` | `host`, `item`, `params`, `value` and `at`, when the value was measured. |

A `graph` or `value` panel on an item that has a parameter (`disk.*` has `mount`, `net.*` has
`interface`) must name it in `params`. A panel without it cannot show one series, so its data is
`null` (`points`, or `value` and `at`); this is not a fault of the metrics store, and
`metrics_available` stays `true`. The same holds for a panel on a host that the dashboard does not
show.

The data of the panels never makes the request fail. When the metrics store cannot be read, or
does not answer within five seconds, the data of the affected panels is `null` and
`metrics_available` is `false`; the problems and host states are still there. Current values lag by
about 30 seconds, see [Conventions](#conventions).

```sh
curl -sS -b cookies.txt http://127.0.0.1:18443/api/v1/dashboards/e2e
```

```json
{"id":"e2e","name":"E2E","file":"dashboards/e2e.json","priority":0,"hosts":[{"name":"e2e-host","state":{"severity":"critical","acknowledged":false,"kind":"no_data","trigger":"Agent silent","instance":"","summary":"no data for 1m","open_problems":2}}],"panels":[{"type":"problems","x":0,"y":0,"w":8,"h":6,"problems":[{"id":"0c208a84-6643-46d4-86ab-066f04916ecc","host":"e2e-host","trigger":"Agent silent","instance":"","severity":"critical","kind":"no_data","summary":"no data for 1m","started_at":"2026-11-16T21:54:05.114Z","resolved_at":null,"resolve_reason":null,"acknowledged_at":null,"acknowledged_by":null,"ack_comment":null,"closed_by":null,"close_comment":null}],"more":0},{"type":"host-states","x":8,"y":0,"w":4,"h":3,"total":1,"without_problems":0,"hosts":[{"name":"e2e-host","state":{"severity":"critical","acknowledged":false,"kind":"no_data","trigger":"Agent silent","instance":"","summary":"no data for 1m","open_problems":2}}]},{"type":"graph","x":8,"y":3,"w":4,"h":3,"host":"e2e-host","item":"cpu.usage","params":{},"range":"1h","step_secs":15,"points":[[1794865935000,28.2028656006],[1794865950000,27.432800292968]],"thresholds":[{"trigger":"E2E always","severity":"high","op":"<","value":0.0}]}],"metrics_available":true}
```

(One problem and two points are shown; the real answer has all of them.)

### Displays

### `GET /api/v1/displays` (viewer)

All wall displays, for the desk (not for the monitors). Not paged. `show` is what the display shows
when nothing is wrong: `{"dashboard": "<id>"}` or `{"rotate": [...], "every": "30s"}`.
`switch_on_alarm` is `null` or `{"min_severity": ...}`; `dashboards` limits which dashboards an
alarm may switch to (`null`: all). `token_created_at` says when the display's token was created,
`null` if it has none; the token itself is never shown again. `target`, `causes` and `waiting` are
what the display shows because of an alarm right now (see the next route).

```json
{"items":[{"id":"e2e-wall","name":"E2E wall","file":"displays/e2e-wall.json","show":{"dashboard":"e2e"},"switch_on_alarm":{"min_severity":"warning"},"dashboards":["e2e"],"token_created_at":null,"target":"e2e","causes":["0c208a84-6643-46d4-86ab-066f04916ecc"],"waiting":[]}]}
```

### `POST /api/v1/displays/{id}/token` (admin)

Creates the token of a display and answers `201`. The token is in this answer and nowhere
else. A token that existed is replaced. `404` for a display that is not in the configuration,
or while a synchronisation is changing it (repeat the request). See [Display tokens](#display-tokens).

```sh
curl -sS -b cookies.txt -X POST http://127.0.0.1:18443/api/v1/displays/e2e-wall/token
```

```json
{"token":"cbx_dt_<token>"}
```

### `DELETE /api/v1/displays/{id}/token` (admin)

Revokes the token. Answer `204`, also when the display has no token or does not exist. A display
whose id cannot be one is `404`.

### `GET /api/v1/display/{id}` (display)

What a wall monitor needs on each poll. Access: a logged-in user of any role, or the token of
this display in `Authorization: Bearer <token>`.

| Field | Meaning |
| --- | --- |
| `id`, `name` | The display. |
| `show`, `switch_on_alarm` | Its setting from the configuration. |
| `allowed` | The dashboards the display may ever show: those in `show`, plus, with `switch_on_alarm`, those in `dashboards` (all dashboards if it has no list). |
| `names` | The name of each dashboard in `allowed` that exists, by id: `{"<id>": "<name>"}`. A dashboard in `allowed` that is not in the configuration has no entry. |
| `target` | The dashboard to show because of an alarm, or `null` to follow `show`. An alarm is an open problem nobody has acknowledged, at or above `min_severity`, on a host of the dashboard. The dashboard with the highest `priority` wins, then the most urgent alarm. |
| `causes` | The ids of the alarms of `target`, most urgent first. |
| `waiting` | The other dashboards that have an alarm, in the order the display would take them. |
| `sync` | The sync state of the configuration, the same object as `sync` of [`/api/v1/status`](#get-apiv1status-viewer): `status` (`never`, `in_sync` or `out_of_sync`), `active_commit` (the commit whose configuration is in use), `last_success_at`, `last_attempt_at` and `issues`, the list of what the last synchronisation found wrong (`file`, `line`, `column`, `reason`). The wall shows "out of sync since ..., showing the configuration from commit ..." with them. |

```sh
curl -sS -H @display.header http://127.0.0.1:18443/api/v1/display/e2e-wall
```

```json
{"id":"e2e-wall","name":"E2E wall","show":{"dashboard":"e2e"},"switch_on_alarm":{"min_severity":"warning"},"allowed":["e2e"],"names":{"e2e":"E2E"},"target":"e2e","causes":["0c208a84-6643-46d4-86ab-066f04916ecc"],"waiting":[],"sync":{"status":"in_sync","active_commit":"253a2994b13aca387e272ff08db9eb858b436b66","last_success_at":"2026-11-16T21:54:32.810398Z","last_attempt_at":"2026-11-16T21:54:50.503921Z","issues":[]}}
```

(`display.header` is a file with the one line `Authorization: Bearer <token>`.) Poll every 5 to
10 seconds.

### `GET /api/v1/display/{id}/dashboards/{dashboard}` (display)

The answer of `GET /api/v1/dashboards/{id}`, for a dashboard in the display's `allowed` list. The
same access as the route above. The route first looks the display up: an unknown display is `404`
for a logged-in user (a token of a display that does not exist is `401`). Then it checks the
dashboard against `allowed`: any dashboard that is not in `allowed`, one that does not exist
included, is `403 forbidden`. `404` comes only when `allowed` names a dashboard that no longer
exists (for example a display that shows a dashboard whose file was removed).

For a request with the display's token, every problem in the answer has `acknowledged_by`,
`ack_comment`, `closed_by` and `close_comment` set to `null` (see [Display tokens](#display-tokens));
everything else, `acknowledged_at` included, is as for a logged-in user.

```json
{"error":"forbidden","message":"this display may not show that dashboard"}
```

### Status and configuration

### `GET /api/v1/status` (viewer)

What a page header needs on each refresh. `sync.status` is `never`, `in_sync` or `out_of_sync`;
`sync.issues` lists why the last synchronisation failed (`file`, `line`, `column` and `reason`;
all but `reason` can be `null`). `mail` is true when e-mail is set up. `hosts` counts the
active hosts, `problems` the open problems per severity.

```json
{"version":"0.1.0","gitops":true,"mail":true,"sync":{"status":"in_sync","active_commit":"253a2994b13aca387e272ff08db9eb858b436b66","last_success_at":"2026-11-16T21:54:32.810398Z","last_attempt_at":"2026-11-16T21:54:50.503921Z","issues":[]},"hosts":{"total":3,"with_problems":1,"without_problems":2,"never_seen":2},"problems":{"critical":{"open":1,"unacknowledged":1},"high":{"open":0,"unacknowledged":0},"warning":{"open":1,"unacknowledged":0},"info":{"open":0,"unacknowledged":0}}}
```

### `GET /api/v1/sync/history` (viewer)

The applied revisions, newest first, paged (`limit`, `offset`). `changes` names the hosts,
templates, notifications, dashboards and displays added, changed and removed.

```json
{"items":[{"commit":"253a2994b13aca387e272ff08db9eb858b436b66","applied_at":"2026-11-16T21:52:57.002426Z","changes":{"hosts":{"added":[],"changed":["e2e-host"],"removed":[]},"templates":{"added":[],"changed":[],"removed":[]},"notifications":{"added":[],"changed":[],"removed":[]},"dashboards":{"added":[],"changed":[],"removed":[]},"displays":{"added":[],"changed":[],"removed":[]}}}],"total":3}
```

### `GET /api/v1/unknown-agents` (operator)

Agents that tried to enrol under a name that is not a registered host: `host_name`, `first_seen`,
`last_seen` and `attempts`. The 500 most recent, not paged.

```json
{"items":[]}
```

### `GET /api/v1/enrollment-keys` (admin)

The enrolment keys, newest first: `id`, `description`, `created_at`, `expires_at`, `revoked_at` and
`state` (`active`, `expired` or `revoked`). The key itself is never shown again. Not paged.

```json
{"items":[{"created_at":"2026-11-16T21:55:22.816219Z","description":"web-02","expires_at":"2026-11-17T21:55:22.815866Z","id":"d5293479-d2de-4693-9da5-99c726925d97","revoked_at":null,"state":"active"}]}
```

### `POST /api/v1/enrollment-keys` (admin)

Creates a key and answers `201`. Body: `description` (1 to 200 characters, not only spaces, no control character) and
`expires_in` (`1h` to `365d`, for example `90m`, `12h` or `30d`). The key is in the `key` field of
this answer only.

```sh
curl -sS -b cookies.txt -X POST -H 'Content-Type: application/json' \
     -d '{"description":"web-02","expires_in":"24h"}' http://127.0.0.1:18443/api/v1/enrollment-keys
```

```json
{"id":"d5293479-d2de-4693-9da5-99c726925d97","description":"web-02","created_at":"2026-11-16T21:55:22.816219Z","expires_at":"2026-11-17T21:55:22.815866Z","revoked_at":null,"state":"active","key":"cbx_ek_<key>"}
```

### `DELETE /api/v1/enrollment-keys/{id}` (admin)

Revokes a key. Answer `204`, also for a key that is already revoked. `404` for an unknown id.

### `POST /api/v1/hosts/{name}/release` (admin)

Removes the agent binding of a host so that an agent can enrol again (the same as
`admin host release`). Answer `204`. The version the page shows as the agent's (`agent_version`) is cleared, too. `404` for an unknown host. It is allowed with GitOps on: it
changes no configuration.

### Users

### `GET /api/v1/users` (admin)

The users, paged (`limit`, `offset`). A user has `id`, `name`, `role`, `disabled`, `created_at`
and `password_changed_at`. There is no password or hash in any answer.

```json
{"items":[{"id":"a35d0a8d-98af-400d-97d7-f6ab7b485fed","name":"docs-admin","role":"admin","disabled":false,"created_at":"2026-11-16T21:54:51.061431Z","password_changed_at":"2026-11-16T21:54:51.061431Z"}],"total":2}
```

### `POST /api/v1/users` (admin)

Creates a user and answers `201`. Body: `name` (1 to 63 characters from `a-z`, `0-9`, `.`, `_` and
`-`, starting with a letter or digit), `role` (`viewer`, `operator` or `admin`) and `password`
(optional, 12 to 256 characters, no NUL). Without `password` the server generates one of 20
characters and returns it in the field `password` of this answer, once. `409 conflict` if the
name is taken. A busy server answers `429 too_many_attempts` and creates nothing, see
*Busy server* under [Login and sessions](#login-and-sessions).

```sh
curl -sS -b cookies.txt -X POST -H 'Content-Type: application/json' \
     -d '{"name":"anna","role":"operator"}' http://127.0.0.1:18443/api/v1/users
```

```json
{"id":"d0e7fedb-ed62-4568-a7b7-6a28a0f0f3b5","name":"anna","role":"operator","disabled":false,"created_at":"2026-11-16T21:55:21.698423Z","password_changed_at":"2026-11-16T21:55:21.698423Z","password":"<password>"}
```

### `PATCH /api/v1/users/{id}` (admin)

Changes the role, disables or enables a user, or both. Body: `role` and/or `disabled`; at least one.
Changing the role or disabling ends the user's sessions. The last enabled admin cannot be demoted or
disabled: `409 conflict`. `404` for an unknown id. The answer is the user.

```sh
curl -sS -b cookies.txt -X PATCH -H 'Content-Type: application/json' \
     -d '{"role":"viewer"}' http://127.0.0.1:18443/api/v1/users/d0e7fedb-ed62-4568-a7b7-6a28a0f0f3b5
```

```json
{"id":"d0e7fedb-ed62-4568-a7b7-6a28a0f0f3b5","name":"anna","role":"viewer","disabled":false,"created_at":"2026-11-16T21:55:21.698423Z","password_changed_at":"2026-11-16T21:55:21.698423Z"}
```

### `POST /api/v1/users/{id}/password` (admin)

Sets a new password for a user and ends all of the user's sessions, those of the caller included
when the caller resets the own password. It also lifts a lock. Body: `password`, optional, like at
creation. Without it the answer has a generated password, once; with it the answer is `{}`.
`404` for an unknown id. A busy server answers `429 too_many_attempts` and changes nothing, see
*Busy server* under [Login and sessions](#login-and-sessions).

```sh
curl -sS -b cookies.txt -X POST -H 'Content-Type: application/json' -d '{}' \
     http://127.0.0.1:18443/api/v1/users/d0e7fedb-ed62-4568-a7b7-6a28a0f0f3b5/password
```

```json
{"password":"<password>"}
```

### `DELETE /api/v1/users/{id}` (admin)

Deletes a user and the user's sessions. Answer `204`. The last enabled admin cannot be deleted:
`409 conflict`. `404` for an unknown id.

## What is not there

- **No push channel.** There are no WebSockets and no server-sent events. Clients poll; a wall
  display every 5 to 10 seconds.
- **No write access to the configuration.** Hosts, templates, notifications, dashboards and
  displays are changed in the Git repository, see
  [The configuration repository](configuration-repository.md). The API reads them. Without
  GitOps there is no way to change them through the API either; hosts are added with
  `cerbrix-server admin host add`.
- **No API versioning promise before 1.0.** `/api/v1` can change between releases until Cerbrix
  1.0. After that, changes that break a client go to a new version.
- **No other ways to log in.** There is no API key, no single sign-on and no way to tell a locked
  user from a wrong password. Admins create users with the API or `cerbrix-server admin user`.
