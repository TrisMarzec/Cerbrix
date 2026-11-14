# Cerbrix – Design

Date: 2026-11-14 · Status: approved by the project owner

Markers: **[D]** = decided by the project owner, **[A]** = author's call, accepted with the design.

## 1. Intent

Cerbrix is an open-source monitoring system written in Rust that monitors remote hosts in the cloud and on-prem, comparable to Zabbix. **[D]**

The reason for a new tool: the whole monitoring configuration (hosts, items, triggers, alerting) can live as JSON files in a Git repository. Cerbrix reconciles that repository periodically at runtime and applies changes by itself. When this mode is active, Git is the single source of truth. **[D]**

The audience is operators who install Cerbrix themselves. **[D]**

## 2. Goals, non-goals, anti-goals

**Goals**

- Monitor base metrics (CPU, memory, storage) on remote hosts through a dedicated agent. **[D]**
- GitOps configuration with JSON files and periodic reconciliation. **[D]**
- Alerting: fired triggers show up as problems on the dashboard and optionally send an e-mail. **[D]**
- A modern web UI that can be used without training. **[D]**
- Installable by third parties with reasonable effort, including documentation. **[D]**

**Non-goals (for the start)**

- Agentless checks (SNMP, SSH, cloud APIs). Later. **[D]**
- Writing UI changes back to Git. Possible later. **[D]**
- Log analysis, tracing, application performance monitoring. **[A]**
- Multi-tenancy (several separate organisations in one installation). **[A]**

**Anti-goals**

- An installation where GitOps is active and the running configuration silently differs from the repository.
- Credentials or keys in the configuration repository.
- A UI that cannot be used without a manual.

## 3. Constraints

- Language: Rust for server, agent and proxy. **[D]**
- Configuration format: JSON. **[D]**
- License: Apache-2.0. **[D]**
- English for code, documentation and issues. **[D]**

## 4. Approaches considered

| Approach | Description | Trade-off |
|---|---|---|
| 1. Single binary | Server with embedded UI and SQLite | Easiest install, carries only a few dozen hosts |
| 2. Server plus PostgreSQL | One server program, data in PostgreSQL/TimescaleDB | Hundreds to a few thousand hosts, one extra service |
| 3. Distributed | Server, dedicated metrics database and proxies as separate services | Scales like a large Zabbix setup, highest build and install effort |

**Chosen: approach 3.** **[D]** The author had recommended approach 2 (faster to a visible result, simpler install); the project owner chose the distributed design.

## 5. Architecture

### 5.1 Components

| Component | Responsibility |
|---|---|
| `cerbrix-agent` | Runs on the monitored host, measures items, sends values. Always opens the connection itself. **[D]** |
| `cerbrix-proxy` | Relay in isolated networks. Accepts values from agents, buffers them during outages, forwards them to the server. Passes configuration from the server to agents. |
| `cerbrix-server` | Accepts values, evaluates triggers, manages problems and notifications, runs the Git sync, serves the API and the UI. |
| Metrics database | Stores time series. **[A]** VictoriaMetrics (single binary, Apache-2.0, efficient storage). Wrapped behind an interface in the code so it stays replaceable. |
| State database | Stores hosts, templates, triggers, problems, users, agent credentials, sync log. **[A]** PostgreSQL. |
| Web UI | **[A]** Single-page application in TypeScript, shipped embedded in the server binary. Framework is the builder's choice. |

```
 Host A ─ agent ─┐
 Host B ─ agent ─┼──────────────► server ──► metrics DB (time series)
                 │                  │  └───► state DB (PostgreSQL)
 Host C ─ agent ─┴─► proxy ────────►│
 (isolated network)                 ├──◄ Git repo (periodic sync)
                                    └──► browser (UI), e-mail
```

### 5.2 Server instances **[D]**

Cerbrix starts with one server instance, but the server must be able to grow beyond that. Consequences for the build from the first milestone on:

- No state that exists only in the memory of one server process: trigger state, problems, sync state and agent credentials live in the databases.
- Value ingest is stateless, so several instances can sit behind a load balancer later.
- Jobs that must run once (Git sync, sending notifications) are written so that a lock or leader election can be added without restructuring.
- Sizing target for a single instance **[A]**: 5,000 hosts.

Actually running several instances in parallel is a later milestone.

### 5.3 Repository layout **[A]**

A Rust workspace with the crates `cerbrix-server`, `cerbrix-agent`, `cerbrix-proxy` and `cerbrix-common` (shared data formats for configuration and transport). The UI lives in its own `web/` directory.

### 5.4 Transport agent → server **[A]**

- Encrypted over HTTPS; the agent (or proxy) opens the connection, the server never connects to a host.
- Towards the agent, the proxy offers the same interface as the server. An agent does not need to know which of the two it talks to.
- The agent buffers values locally when its peer is unreachable and delivers them later.
- The exact wire format is the builder's choice.
- **TLS:** the operator supplies the certificate (on the server directly or through a reverse proxy in front). The agent can be given a custom CA file. Cerbrix does not build its own auto-generated CA.

### 5.5 Agent enrolment **[A]**

- At install time the agent gets the host name from the host file and an enrolment key.
- The key is not stored in the Git repository. It is created in the server (UI or command line) and stored there only as a hash.
- An enrolment key is reusable, has an expiry date and can be used for many hosts, so rollout stays automatable. On first contact the agent exchanges it for its own credential that is valid only for its host name. If a host name is already bound to an agent, a second enrolment is rejected until an administrator releases the binding in the UI. When the host file disappears, the credential is revoked.
- An agent that reports a host name without an entry is rejected and listed in the UI under "Unknown agents".
- The agent fetches its configuration (items, intervals) from the server. A change in the repository takes effect after the next sync without touching the host.

### 5.6 Configuration model

Following the Zabbix model: **host**, **item** (what is measured), **trigger** (when it is a problem). **[D]**

Example host file:

```json
{
  "host": "web-01",
  "address": "10.0.4.12",
  "groups": ["production", "webserver"],
  "templates": ["linux-base"],
  "items": [
    { "key": "cpu.usage", "interval": "30s" },
    { "key": "disk.used_percent", "params": { "mount": "/" }, "interval": "5m" }
  ],
  "triggers": [
    {
      "name": "Disk almost full",
      "expression": "disk.used_percent{mount=/} > 90 for 10m",
      "severity": "high"
    }
  ]
}
```

- **`address`** is informational only (shown in the UI). The server does not check the sender address against it, because that would break behind NAT and proxies.
- **Multi-instance items:** items such as disk usage per mount point or throughput per network interface apply, when no `params` are given, to every instance the agent finds on the host. The agent reports the discovered instances itself; pseudo file systems and loopback are excluded by default, further exclusions are configurable by pattern. A trigger on such an item applies per instance and creates one problem per instance. `params` narrows an item or trigger to one instance. This is what lets templates work without knowing each host's mount points.
- **Templates** are separate JSON files with items and triggers. A host includes templates and adds or overrides individual entries. On overlap the host entry beats the template; with several templates the one listed later wins.
- **Notifications** (who gets an e-mail for which severity and group) are JSON files in the repository as well.
- **Repository layout:** `hosts/*.json`, `templates/*.json`, `notifications/*.json`. A published JSON Schema exists for every file type so editors and CI can validate the files.
- **Trigger expressions** start deliberately small: one item, one comparison (`>`, `<`, `>=`, `<=`, `==`, `!=`), an optional hold duration (`for 10m`), plus an expression for "no data for X". Combining several items and functions such as averages come later. The exact syntax is defined and documented by the builder.
- **Severities:** `info`, `warning`, `high`, `critical`.
- **Initial items:** CPU usage, load, memory used/free, swap, disk usage per mount point, network throughput per interface, uptime, agent reachable.

### 5.7 Git sync

- GitOps is a switch for the whole installation. When on, hosts, templates and notifications come exclusively from the repository; the UI shows them read-only and marked as "managed by Git". When off, they are managed in the UI. No mixed mode. **[D]**
- The server fetches the repository on a configurable cycle (default 60 seconds) from a configurable branch. Access is read-only, via HTTPS token or SSH key.
- **All or nothing:** a revision is applied only if all files are valid (schema, known templates, unique host names, valid trigger expressions). If a revision is invalid, the last valid configuration stays active.
- **Out Of Sync [D]:** whenever a sync attempt fails, the installation is in the state *Out Of Sync*. The dashboard must show this state prominently and name the file in which the error lies, together with the reason (and the position in the file where available). The state also covers failures without a file, such as an unreachable repository or rejected credentials; then the reason is shown instead. It clears automatically with the next successful sync.
- The UI shows the active commit, the time of the last successful sync, the error of the last attempt and the history of applied changes.
- **Removed hosts:** when a host file disappears, the host is deactivated, its open problems are closed and its agent is rejected. Stored metric values remain until retention expires.

### 5.8 Trigger evaluation and problems

- The server evaluates triggers as values arrive.
- When a trigger's condition holds, a **problem** is created with host, trigger, severity and start time. When the condition no longer holds, the problem is closed automatically.
- **Late values:** every value carries its measurement time and is stored with it, so graphs have no gaps after an outage. Triggers only evaluate values that are at most a few minutes old (configurable); older, late-delivered values do not open or close problems retroactively and send no e-mails. "No data for X" is based on arrival at the server.
- Problems appear on the dashboard. **[D]**
- Users can acknowledge a problem with a comment.
- E-mail through a configurable SMTP server when a problem starts and when it ends, according to the notification files. **[D]** Sending is built as a replaceable channel; webhook and chat services follow later.

### 5.9 UI

Views for the start:

- **Dashboard:** open problems by severity, number of hosts by state, Git sync status including the Out Of Sync message with file and reason.
- **Hosts:** list with search and filters for group and state; detail page with current values, history graphs per item, triggers and problems of the host.
- **Problems:** current and past, filterable, with acknowledgement.
- **Configuration:** with GitOps on, shows commit, sync history and errors; list of unknown agents; creation of enrolment keys.
- **Administration:** users and roles.

The concrete look is agreed with the project owner separately before the UI is built.

### 5.10 Users and roles **[A]**

Local users with password. Three roles: `viewer` (read only), `operator` (also acknowledge problems), `admin` (also users, keys, settings). Login through external providers (OIDC/LDAP) later.

### 5.11 Delivery **[A]**

- Docker Compose as the recommended way: server, PostgreSQL and metrics database with one command.
- Agent and proxy as single binaries without dependencies, plus an install script that sets the agent up as a system service.
- Server settings (databases, SMTP, Git repository, cycle) through a configuration file and environment variables.
- Platforms: agent for Linux first, then Windows. Server and proxy for Linux.

## 6. Order of work

The project consists of several independent parts.

| Milestone | Content |
|---|---|
| **M1 – Metrics pipeline** | Workspace, shared data formats, Linux agent with the initial items, server accepts and stores values, agent enrolment, Docker Compose |
| **M2 – Configuration from Git** | JSON Schema, templates, Git sync with validation and Out Of Sync state, agent fetches its configuration |
| **M3 – Alerting** | Trigger evaluation, problems, e-mail |
| **M4 – UI** | Login and roles, dashboard, hosts, problems, configuration view |
| **MVP reached** | M1–M4 together: a host from Git is monitored, a trigger creates a problem on the dashboard and an e-mail |
| **v0.2** | Proxy, Windows agent, management in the UI with GitOps off, retention, several server instances |
| **Later** | Agentless checks, more notification channels, extended trigger expressions, OIDC/LDAP, write-back to Git, SQLite variant for very small installations |

Consequence of this order: in the MVP, GitOps is the only way to manage hosts. The proxy arrives in v0.2, but the agent's interface is built from the start so that a proxy fits in between.

## 7. Left to the builder

- Choice of Rust libraries (web server, database access, Git, system metrics).
- Wire format and API of the agent → server transport.
- Exact syntax of trigger expressions.
- Tables of the state database and naming of time series.
- UI framework and components.
- Test strategy and CI.

## 8. Feature planning

Every feature is an issue in the repository `TrisMarzec/Cerbrix` and is tracked in the GitHub Project "Cerbrix" with status, priority, area and milestone.
