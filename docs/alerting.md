# Alerting

Cerbrix watches the values that its agents send. When a value shows that something is wrong, it
opens a **problem** and sends an e-mail. When the value is fine again, the problem closes and a
second e-mail says so. This page explains what you write in the configuration repository, what
the server does with it, and what you see.

What is not there yet, so that you do not look for it:

- There is **no web interface and no HTTP API for problems**. You list, acknowledge and close
  problems with `cerbrix-server admin problem ...` commands. The interface comes with the
  next milestone (M4).
- **E-mail is the only channel.** No chat, SMS or webhook.
- E-mails are **plain text in English**, with times in **UTC**.
- A host whose clock is badly wrong is not alarmed about; the server only writes a warning to
  its log (see [Values must be recent](#values-must-be-recent)).
- A mail relay whose certificate was issued by your own (internal) certificate authority cannot
  be used with `starttls` or `tls` yet (see [SMTP settings](#smtp-settings)).

## Contents

- [How a trigger becomes a problem](#how-a-trigger-becomes-a-problem)
- [When data stops arriving](#when-data-stops-arriving)
- [When a problem closes](#when-a-problem-closes)
- [Who is told: notification rules](#who-is-told-notification-rules)
- [The e-mails](#the-e-mails)
- [SMTP settings](#smtp-settings)
- [Testing the mail setup](#testing-the-mail-setup)
- [When the mail server is not reachable](#when-the-mail-server-is-not-reachable)
- [Looking at problems: the admin commands](#looking-at-problems-the-admin-commands)
- [Settings for alerting](#settings-for-alerting)
- [How this is tested](#how-this-is-tested)

## How a trigger becomes a problem

A trigger is a rule in a host file or a template. It has a name, an expression and a severity
(`info`, `warning`, `high` or `critical`). The syntax of expressions is described in
[Trigger expressions](configuration-repository.md#trigger-expressions). Here is a host with two
triggers that compare values:

File `hosts/db-02.json`:

```json
{
  "host": "db-02",
  "groups": ["production", "db"],
  "items": [
    { "key": "cpu.usage", "interval": "30s" },
    { "key": "disk.used_percent", "params": { "mount": "/" }, "interval": "1m" },
    { "key": "disk.used_percent", "params": { "mount": "/var" }, "interval": "1m" }
  ],
  "triggers": [
    {
      "name": "Disk almost full",
      "expression": "disk.used_percent > 90 for 10m",
      "severity": "high"
    },
    {
      "name": "CPU saturated",
      "expression": "cpu.usage >= 95 for 15m",
      "severity": "warning"
    },
    {
      "name": "Agent silent",
      "expression": "no data for 3m",
      "severity": "critical"
    }
  ]
}
```

Every time a batch of values arrives from an agent, the server checks the triggers of that host
against it. It does this while it handles the request, so the problem appears within moments of
the value arriving, not on a timer.

**Holding.** `for 10m` means the condition must be true for ten minutes before a problem opens.
The server measures this with the times at which the values were *measured* on the host, not
with the time they arrived. The first value that makes the condition true starts the hold; the
first value that is at least ten minutes later and still true opens the problem. A single value
that makes the condition false starts the hold over. Without `for`, the first value that makes
the condition true opens the problem. A problem's start time is the measurement time of the value
that opened it, after the hold, so it is the moment the condition became an alarm. The hold is
kept in the database: restarting the server in the middle of a hold does not lose it.

**One problem per instance.** An item can have several instances: `disk.used_percent` exists once
per file system. A trigger that does not name a file system, like `disk.used_percent > 90`,
is checked separately for each of them, and each can have its own problem. The e-mail subject
and the problem list then show the instance, for example `(mount=/var)`. Several parameters are
shown as `key=value` pairs sorted by key and separated by commas. If you write the parameter in
the trigger (`disk.used_percent{mount=/var} > 90`), only that instance is watched.

**No duplicates.** A trigger has at most one open problem per host and instance. If an agent
sends the same values twice (it does this after an error), nothing opens twice, and no e-mail is
sent twice. A problem does not flap: while it is open, further values that still meet the
condition change nothing.

**Order of values.** Within one batch, values are looked at in order of their measurement time.
A value that is not newer than the last one the trigger has seen is skipped.

### Values must be recent

Only values that are at most 5 minutes old (measured against the server's clock when they
arrive) can open or close a problem. Older values are stored and drawn in graphs, but they never
open or close a problem and never cause an e-mail. This protects you from a flood of e-mails when
an agent that was offline delivers hours of stored values at once. The limit is
`CERBRIX_ALERTING__MAX_VALUE_AGE_SECS` (default 300, see [Settings for
alerting](#settings-for-alerting)).

If a host's clock is wrong by more than that, all its values count as too old and the host would
never alarm. The server cannot know the right time on that host, so it writes a warning to its log,
at most once per hour per host, whenever a delivery consisted only of values that were too old:

```
WARN cerbrix_server::api::agent: stored values older than the tolerance without evaluating them host=db-02 values=3 tolerance_secs=300
```

This is the only notice. Watch the log, or fix the clock (use NTP) on the host.

## When data stops arriving

A host that is down, or an agent that was stopped, sends no values, so a trigger that compares
values never fires. That is what the **no data** triggers are for.

```
"expression": "no data for 3m"
"expression": "disk.used_percent{mount=/var} no data for 1h"
```

The first is a **host-level** trigger: the host as a whole sent nothing for three minutes. The
second is an **item-level** trigger: the host is reporting, but not that item (for example when
one collector fails).

Points to know:

- **Silence is counted from arrival at the server**, not from the measurement time in the
  values. A host that delivers old values after an outage is not silent.
- **A host counts only from its first value.** A host that is in the repository but whose agent
  was never installed does not alarm. It starts to count as silent after the first value it ever
  sent. An item-level trigger works the same way per instance: it knows an instance (for
  example the mount `/var`) from the first value of it that arrived.
- **An outage is one problem.** If the agent or the host dies, all its items go silent at the
  same time. To get one problem for that and not one per item, a host that has an item-level
  no-data trigger must also have a host-level one. An item-level trigger fires only for an item
  that was already silent for its whole window at the moment the host last reported, that is,
  an item that stopped while the rest of the host went on. When everything stops at once, no
  item-level trigger fires, whatever its window compared with the host-level one, and only the
  host-level trigger reports, at its own window. While the host as a whole is silent, an
  item-level problem that is already open stays open. The configuration is refused when the
  host-level trigger is missing:

  ```
  hosts/db-02.json: trigger "Var silent" watches one item for missing data; add a host-level trigger such as "no data for 5m" so that an agent outage is one problem
  ```

  The host-level trigger may come from a template: the template example in
  [the repository guide](configuration-repository.md#template-files-templates) has "Agent
  silent" for every host that uses it.
- **It is checked on a timer.** The server checks all hosts with no-data triggers every 15
  seconds (`CERBRIX_ALERTING__NODATA_CHECK_SECS`, 5 to 300). A problem therefore opens up to one
  check interval after the window has passed.
- **After a restart, silence counts from the start.** After every start of the server, no-data
  detection counts silence from that start. The agents have not had time to reconnect yet, and a
  restart or an upgrade must not make every host look silent. So after a start of the server,
  no host can be reported silent before its window has passed again. The same protection applies
  later: if the server itself did not run its check for more than three check intervals (the
  database was unavailable, for example), silence counts from the moment it checks again. A
  problem that is already open is never closed by this; it closes only when values arrive
  again.
- **When values arrive again, the problem closes**, at the next check after the first value
  arrived, so up to one check interval later. The problem for a host closes after a value of the
  host arrived (for an item-level trigger: a value of that item, while the host reports).
- **An instance that disappears keeps its problem.** When a file system is unmounted or a
  network interface is removed, no value of it will ever arrive again, so its problem (a no-data
  problem, or for example `disk.used_percent > 90` on a disk that was full) stays open until you
  close it by hand with `admin problem close` (see
  [the admin commands](#looking-at-problems-the-admin-commands)).
- **A removed and restored trigger starts afresh.** When an item-level no-data trigger is
  removed (or its selector narrowed), the server forgets when the items it no longer watches
  last reported. If you restore it later, for example by reverting the commit, it knows an
  instance again from the first value of it that arrives after the restore; until then that
  instance cannot alarm.

An example with both levels:

File `hosts/web-03.json`:

```json
{
  "host": "web-03",
  "groups": ["production", "web"],
  "items": [
    { "key": "cpu.usage", "interval": "30s" },
    { "key": "disk.used_percent", "params": { "mount": "/var" }, "interval": "1m" }
  ],
  "triggers": [
    {
      "name": "Agent silent",
      "expression": "no data for 3m",
      "severity": "critical"
    },
    {
      "name": "Var not reported",
      "expression": "disk.used_percent{mount=/var} no data for 10m",
      "severity": "warning"
    }
  ]
}
```

## When a problem closes

A problem closes, and the end is mailed, for one of these reasons. `admin problem list --all`
and the e-mail give the reason in these words:

| Reason | When |
| --- | --- |
| the condition no longer holds | A new value does not meet the condition any more, or, for a no-data trigger, values arrive again. |
| the host was removed from the configuration | The host's file was deleted from the repository (or the host was otherwise deactivated). |
| the trigger was removed from the configuration | The trigger was deleted or renamed, or it now watches something else (see below). |
| the problem was closed by an operator | Someone closed it with `admin problem close`. |

When a trigger is changed in the repository:

- If you change only the threshold, the operator, the hold duration (`for`), the no-data window or
  the severity, an open problem stays open and the new definition applies from the next value.
- If you change **what the trigger watches**, that is, comparison versus no data, the item key, or
  the parameters in the selector, its open problems are closed with the reason "the trigger was
  removed from the configuration", and the trigger starts afresh. The old condition and the new
  one are different alarms.
- A renamed trigger is a removed trigger plus a new one.

The sync does this when it applies the new revision, in the same step. The e-mail about the end
goes to everyone who was mailed about the start.

## Who is told: notification rules

Notification rules are files in `notifications/`. A rule says which problems go to which
addresses:

File `notifications/ops.json`:

```json
{
  "name": "ops",
  "email": ["ops@example.com", "oncall@example.com"],
  "min_severity": "high",
  "groups": ["production"]
}
```

File `notifications/everything.json`:

```json
{
  "name": "everything",
  "email": ["monitoring@example.com"],
  "min_severity": "info"
}
```

A rule applies to a problem when the problem's severity is at least `min_severity` (the order is
`info`, `warning`, `high`, `critical`) **and** the host is in **all** the groups listed in
`groups`. A rule without `groups` applies to every host. If several rules apply, the addresses
are put together and each address gets one e-mail. The host `db-02` above (groups `production`
and `db`) with a `high` problem is mailed to `ops@example.com`, `oncall@example.com` and
`monitoring@example.com`; a `warning` problem only goes to `monitoring@example.com`. Addresses are
compared exactly as written.

The rules are looked at when a problem **opens**. The end of the problem is sent to the same
addresses, whatever the rules say by then, so nobody is left with an alarm that never ends.
Acknowledging a problem does **not** stop the e-mail about its end.

If no rule applies, the problem is still recorded and can be listed, but no e-mail is sent.

## The e-mails

Subjects:

| Event | Subject |
| --- | --- |
| A problem starts | `[Cerbrix] <SEVERITY> <host>: <trigger name>` |
| ... with an instance | `[Cerbrix] <SEVERITY> <host>: <trigger name> (<instance>)` |
| A problem ends | `[Cerbrix] Resolved <host>: <trigger name>` (and ` (<instance>)` when there is one) |

`<SEVERITY>` is `INFO`, `WARNING`, `HIGH` or `CRITICAL`. A subject has at most 200 characters.

This is the text of a start (copied from a message the end-to-end check received):

```
Subject: [Cerbrix] HIGH e2e-host: E2E always

Host: e2e-host
Trigger: E2E always
Severity: HIGH
State: problem
Detail: cpu.usage >= 0: value 2.889
Started: 2026-11-15T22:58:13Z

Sent by Cerbrix.
```

and of the end of the same problem:

```
Subject: [Cerbrix] Resolved e2e-host: E2E always

Host: e2e-host
Trigger: E2E always
Severity: HIGH
State: resolved
Detail: cpu.usage >= 0: value 2.889
Started: 2026-11-15T22:58:13Z
Ended: 2026-11-15T22:59:03Z
Duration: 49 s
Reason: the condition no longer holds

Sent by Cerbrix.
```

A problem with an instance has an additional `Instance: mount=/var` line after the trigger. The
`Detail` line repeats the expression and the value that opened the problem (a no-data problem
shows, for example, `no data for 3m`). Times are UTC.

## SMTP settings

E-mail is turned on by setting the address of a mail server. Without `CERBRIX_SMTP__HOST` no
e-mail is sent: problems are still recorded, and a problem that opens while SMTP is not
configured is never mailed, not even after you configure it later. Put the settings in
`deploy/server.env` (the example file has them, commented out) and apply them with
`docker compose up -d`.

| Variable | Default | Meaning |
| --- | --- | --- |
| `CERBRIX_SMTP__HOST` | none (e-mail off) | Name of the mail server. |
| `CERBRIX_SMTP__PORT` | `587` | Port. Use 587 for `starttls`, 465 for `tls`. |
| `CERBRIX_SMTP__ENCRYPTION` | `starttls` | `starttls`: the connection is upgraded with STARTTLS, and the server refuses to send in plain text if that fails. `tls`: TLS from the first byte. `none`: plain text, only for a trusted local relay. Certificates are always verified. |
| `CERBRIX_SMTP__FROM` | none, **required** when a host is set | The sender, for example `Cerbrix <cerbrix@example.com>`. |
| `CERBRIX_SMTP__USERNAME` | none | User name for the mail server. Set together with the password, or not at all. |
| `CERBRIX_SMTP__PASSWORD` | none | Password. Never written to the log or the database. |
| `CERBRIX_SMTP__TIMEOUT_SECS` | `15` | Seconds allowed for sending one message, 1 to 120. |

Example for a mail server with a login:

```
CERBRIX_SMTP__HOST=smtp.example.com
CERBRIX_SMTP__PORT=587
CERBRIX_SMTP__ENCRYPTION=starttls
CERBRIX_SMTP__FROM=Cerbrix <cerbrix@example.com>
CERBRIX_SMTP__USERNAME=cerbrix@example.com
CERBRIX_SMTP__PASSWORD=the-password
```

A wrong combination stops the server at start with a message that never contains the password,
for example `smtp.from is required when smtp.host is set`, `smtp.username requires
smtp.password` or `smtp.port must not be 0`. Leave a setting out instead of leaving it empty:
an empty user name or password is rejected.

Two limitations:

- **Only the public certificate authorities are trusted** for the TLS connection to the mail
  server. A relay with a certificate from an internal CA cannot be used with `starttls` or `tls`
  yet. (A relay on a trusted network can use `none`.)
- **Credentials are refused with `encryption = "none"`.** The server does not start with
  `smtp.encryption = "none" would send the password unencrypted`, because that would send the
  password over the network in plain text.

## Testing the mail setup

Send a test message before you rely on it:

```sh
docker compose exec server cerbrix-server admin notify test you@example.com
```

```
test message accepted by mailpit:1025
```

The message has the subject `[Cerbrix] INFO cerbrix-test: Test message` and says it is a test.
"Accepted" means that the mail server took the message over; whether it arrives is up to it. The
command works only with SMTP settings: without them it fails with
`no SMTP server configured (smtp.host)`.

## When the mail server is not reachable

A message to send is first written to the database, in the same step that opens or closes the
problem. A separate task delivers it, so a mail server that is down does not hold anything up and
a restart of the server loses nothing. If a message cannot be sent, it is tried again later, with
pauses that start at 1 minute and double each time, up to one hour (1, 2, 4, ... 60 minutes).

How long it is tried depends on the error:

- **Retried for 24 hours:** connection errors and timeouts, TLS errors (including a certificate
  that cannot be verified, and a server that does not offer STARTTLS), temporary failures of the
  mail server (answers starting with 4), and failed logins (answers 530, 534, 535 and 538). A wrong
  password is a problem of the setup, which you will fix, so the alerts wait for you.
- **Retried for one hour, then given up:** other permanent refusals, for example an address that
  the mail server rejects. Trying such a message for a day would not help.

When a message is given up, the server writes an error to its log (`notification given up after
24 hours of failed attempts` or `... after one hour of permanent errors`) and the message is not
sent. The e-mail about the end of a problem is never sent before the e-mail about its start while
that one is still waiting, so the two arrive in the right order after an outage. When the e-mail
about the start was given up, the e-mail about the end is still sent: it names the start, the end
and the duration, and it may be the only notice a recipient gets.

A message is normally sent once, but delivery is "at least once", not "exactly once": if the
server stops or loses its database between the mail server accepting a message and the server
recording that it was sent, or if the time allowed for a send runs out after the mail server
had already accepted it, the message is sent again later. Duplicates are possible in those
cases; lost messages are not.

To see the state of the queue:

```sh
docker compose exec server cerbrix-server admin notify status
```

```
pending: 0
sent in the last 24 hours: 4
failed in the last 24 hours: 0
oldest pending: none
```

If something waits, `oldest pending` shows when it was created, the address, how many attempts
were made and the last error.

## Looking at problems: the admin commands

Run the commands in the server container. Log lines go to the error output; add `2>/dev/null` in
scripts.

`admin problem list` shows the open problems, the most severe first. Each line has the start time
(UTC), the severity, the host, the trigger (with the instance), the state and the first 8
characters of the id:

```sh
docker compose exec server cerbrix-server admin problem list
```

```
2026-11-15T23:00:09Z  CRITICAL  e2e-host  Agent silent  open  f8ed5ec3
2026-11-15T22:58:08Z  WARNING  e2e-host  E2E second  acknowledged by admin  d33ac168
```

With `--all` resolved problems are listed too, newest first; a resolved one shows when and why it
ended. `--limit N` shows at most N lines (1 to 1000, default 50).

```
2026-11-15T23:00:09Z  CRITICAL  e2e-host  Agent silent  open  f8ed5ec3
2026-11-15T22:58:13Z  HIGH  e2e-host  E2E always  resolved 2026-11-15T22:59:03Z (the condition no longer holds)  e18f7225
2026-11-15T22:58:08Z  WARNING  e2e-host  E2E second  acknowledged by admin  d33ac168
```

`admin problem show <id>` prints every field of one problem. The id is the full id or at least
its first 4 hexadecimal characters, if that is enough to tell it from the others:

```sh
docker compose exec server cerbrix-server admin problem show d33ac168
```

```
Id: d33ac168-0f8b-4278-8b1c-f094dbaced14
Host: e2e-host
Trigger: E2E second
Severity: WARNING
Detail: memory.used_percent >= 0: value 42.867
Started: 2026-11-15T22:58:08Z
Acknowledged: 2026-11-15T22:59:05Z by admin
Comment: e2e check
```

`admin problem ack <id> --comment "..." [--by <name>]` acknowledges a problem: it says "someone is
on it". The comment is required; `--by` defaults to `admin`. Acknowledging changes the state shown
in the list. It does **not** close the problem and does **not** stop the e-mail when the problem
ends.

```sh
docker compose exec server cerbrix-server admin problem ack d33ac168 --comment "disk is being replaced" --by anna
```

```
problem d33ac168-0f8b-4278-8b1c-f094dbaced14 acknowledged
```

`admin problem close <id> --comment "..." [--by <name>]` closes an open problem by hand. Use it
for a problem that can never end by itself because its instance is gone: a file system that was
unmounted, a network interface that was removed. The problem is resolved with the reason "the
problem was closed by an operator", the comment and the name are recorded as its
acknowledgement (unless it was acknowledged already), the server forgets the state of that
trigger and instance, and the e-mail about the end goes to everyone who was mailed about the
start (when SMTP is configured). If the condition still holds, for example the disk is still
mounted and still full, the next value opens a new problem. Closing a problem that is already
resolved fails with `problem <id> is already resolved`.

```sh
docker compose exec server cerbrix-server admin problem close f8ed5ec3 --comment "USB disk unmounted" --by anna
```

```
problem f8ed5ec3-2c1a-4f0e-9d5b-7a61c0e4b2d9 closed
```

An unknown id gives `no problem with id ...`; an id of fewer than 4 characters or one that matches
several problems is refused with a message that says so.

## Settings for alerting

| Variable | Default | Allowed | Meaning |
| --- | --- | --- | --- |
| `CERBRIX_ALERTING__MAX_VALUE_AGE_SECS` | `300` | 10 to 86400 | Values older than this are stored but never open or close a problem. |
| `CERBRIX_ALERTING__NODATA_CHECK_SECS` | `15` | 5 to 300 | How often the server checks for silent hosts and items. |

The [SMTP settings](#smtp-settings) are described above. If you run more than one server
instance, they reach the same decisions: the state of triggers, problems and queued e-mails is in
PostgreSQL, and two instances never work on the same message at the same time. Delivery is at
least once, as described in [When the mail server is not
reachable](#when-the-mail-server-is-not-reachable).

## How this is tested

`scripts/e2e.sh` (run by CI on every push) starts a throw-away stack with a real mail server,
[Mailpit](https://mailpit.axllent.org/), and checks the chain from the agent to the inbox:

1. A trigger that is always true opens a problem, and Mailpit receives one message to
   `ops@example.com` with the subject `[Cerbrix] HIGH <host>: E2E always`.
2. A commit that changes the trigger so that it is never true closes the problem, and Mailpit
   receives `[Cerbrix] Resolved <host>: E2E always`.
3. A second problem is acknowledged with `admin problem ack` and the list shows
   `acknowledged by admin`.
4. The agent is stopped. Within minutes the host-level no-data problem opens and its e-mail
   arrives.
5. No message was sent twice.

When it fails, the script prints `admin problem list --all`, `admin notify status` and the
messages in Mailpit.

**What it does not cover:** Mailpit is used without TLS and without a login. The real
connection to your mail server (STARTTLS or TLS, the certificate, the user name and password)
is not tested by it. Check that with `admin notify test <address>` on your installation, as
described in [Testing the mail setup](#testing-the-mail-setup).
