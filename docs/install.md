# Installing the Cerbrix server

This guide installs the Cerbrix server together with PostgreSQL and VictoriaMetrics
using Docker Compose. The quick start brings the stack up reachable from the local
machine only; [Making the server reachable for agents](#making-the-server-reachable-for-agents)
then opens it up safely.

## Prerequisites

- A Linux host (or any host that runs Docker) with Docker and the Docker Compose plugin
- A DNS name for the server that your monitored hosts can reach
- A TLS certificate for that name, or a reverse proxy that provides one

## 1. Get the sources and configure

```sh
git clone https://github.com/TrisMarzec/Cerbrix.git
cd Cerbrix
cp deploy/server.env.example deploy/server.env
```

Edit `deploy/server.env` and set a database password. The example ships with an empty
password on purpose: PostgreSQL refuses to start until you choose one.

- Put the same password in `POSTGRES_PASSWORD` and in `CERBRIX_DATABASE__URL` (between
  `cerbrix:` and `@postgres`).
- The password is embedded in a URL, so it must be URL-safe: use a long random string of
  letters and digits, for example `openssl rand -hex 24`.

`deploy/server.env` is excluded from Git. Keep it private.

## 2. Start the stack

```sh
docker compose up -d --build
docker compose ps
```

The first start builds the server image, which takes several minutes. The stack consists of
`postgres`, `victoriametrics` (keeps metric values for 12 months by default, see
[Retention](#retention)) and `server`. The server applies its
database migrations on start. By default the server is published on `127.0.0.1:8443` and
VictoriaMetrics on `127.0.0.1:8428` only; PostgreSQL is not published at all. Check it:

```sh
curl http://127.0.0.1:8443/healthz
```

## 3. First login

The web UI is part of the server binary and of its Docker image: there is nothing more to install,
and it is served at `/` of the server (for example `http://127.0.0.1:8443/`). It needs a user; see
[The web UI](web-ui.md) for what you can do with it. Create the first one as an admin:

```sh
docker compose exec server cerbrix-server admin user create t.marzec --role admin
```

The command prints `user t.marzec created with role admin` and a generated password on a
second line. The password is shown only once; store it now. Names use lower case letters,
digits, `.`, `_` and `-`. To choose the password yourself, read it from standard input
(at least 12 characters, the first line is used):

```sh
printf '%s\n' "$PASSWORD" | docker compose exec -T server cerbrix-server admin user create t.marzec --role admin --password-stdin
```

Other commands: `admin user list`, `admin user set-role <name> <viewer|operator|admin>`,
`admin user disable <name>`, `admin user enable <name>`, `admin user reset-password <name>`
(also with `--password-stdin`) and `admin user delete <name>`. Changing a role or password,
disabling or deleting a user ends the user's sessions. The last enabled admin cannot be
demoted, disabled or deleted.

A monitor on the wall needs no user: it opens `https://<server>/display/<id>` for a display
defined in the configuration repository and uses a token that an admin creates in the UI at a
desk. Do not log in on the monitor's browser: a display opened there with a login runs on that
login and stops when it ends. The monitor's browser must keep the token in a persistent profile,
and it must be started by a script that waits for the server's `/healthz`, so that the wall comes
back by itself after a power cut. See [Wall displays](web-ui.md#wall-displays) for setting one up
and [Kiosk mode](web-ui.md#kiosk-mode) for the start script.

### Plain HTTP is for testing only

The quick start serves the UI over plain HTTP. That is fine at `http://127.0.0.1:8443/` on the
machine that runs the server, which browsers treat as secure. Opening the UI over plain HTTP
under any other name (for example `http://monitoring.example.net:8443/`) is for testing only: the
session cookie then has no `Secure` attribute, the browser offers the page no clipboard, so every
"Copy" button asks you to select the text and press Ctrl+C instead, and the browser console shows
a message that the `Cross-Origin-Opener-Policy` header is ignored. Put the server behind HTTPS for
real use (see [Behind a reverse proxy](#behind-a-reverse-proxy) and
[Making the server reachable for agents](#making-the-server-reachable-for-agents)).

### Login settings

Sessions are kept in PostgreSQL, so they survive a restart of the server. These settings
(in `deploy/server.env`, a commented block is in the example file) control them and the
counting of failed logins:

| Variable | Default | Meaning |
| --- | --- | --- |
| `CERBRIX_AUTH__SESSION_IDLE_SECS` | `43200` (12 hours) | A session that was not used for this long ends. 300 to 604800. |
| `CERBRIX_AUTH__SESSION_MAX_SECS` | `604800` (7 days) | A session ends this long after the login, however active it is. Not smaller than the idle time, at most 2592000 (30 days). |
| `CERBRIX_AUTH__SECURE_COOKIE` | unset | Whether the session cookie carries the `Secure` attribute, so that a browser sends it only over HTTPS. Unset means on, unless `CERBRIX_SERVER__INSECURE_HTTP=true`. Set it to `true` when a reverse proxy terminates TLS. |
| `CERBRIX_AUTH__CLIENT_ADDRESS_HEADER` | unset | The request header that names the client address for counting failed logins, such as `X-Forwarded-For`; the right-most address in it is used. Unset means the address of the connection. Set it only behind a reverse proxy, see [Behind a reverse proxy](#behind-a-reverse-proxy). |

An invalid value stops the server at start. After five wrong passwords in a row a user is
locked for five minutes; the lock ends by itself and a login with the right password resets
the count. The login answer is the same for a wrong password, an unknown name and a locked
user. The [API reference](api.md) describes the login and the roles.

Failed logins are also counted per client address (an IPv6 address counts as its /64): after
10 failed logins within 5 minutes, counted from the first of them, further logins from that
address are answered `429 too_many_attempts` until the 5 minutes are over, without checking
the password and without counting against the user. The server logs a warning when it starts
refusing an address. The count is kept in memory and starts afresh when the server restarts.
This limits how fast one address can guess passwords, and how many users it can keep locked. It
does not prevent somebody who knows a user name from keeping that user locked: five wrong
passwords lock a user, fewer than the ten an address may try in 5 minutes, so one address is
still enough for one user. And it offers little against somebody with many addresses, such as a
large IPv6 range, who gets ten tries for each address (for IPv6, each /64).

### Behind a reverse proxy

Pass every path to the server, not only `/api`: the web UI is served at `/` and its pages, such as
`/hosts` or `/problems`, are answered by the server too. That includes `/healthz`, which the start
script of a wall monitor asks before it starts the browser (see
[Kiosk mode](web-ui.md#kiosk-mode)).

The login and every request that changes something are checked against cross-site requests:
when a browser sends an `Origin` header, its host and port must equal those of the `Host`
header. So the proxy must pass the `Host` header on unchanged. In nginx that is
`proxy_set_header Host $http_host;`, in Caddy it is the default. If the proxy replaces `Host`
with the name of the upstream, every login from a browser is answered with 403.

When the proxy terminates TLS and talks plain HTTP to the server, as in
[option (b) below](#making-the-server-reachable-for-agents), set
`CERBRIX_AUTH__SECURE_COOKIE=true`. The server cannot see that the browser uses HTTPS, and
without the setting it sends the cookie without `Secure`. Do not publish the plain HTTP port
of the server to a network; the session cookie and the passwords of a login would cross it
unencrypted.

Failed logins are counted per client address (see [Login settings](#login-settings)). Behind a
proxy, the address of every connection is that of the proxy, so all users share one counter:
10 failed logins by anybody within 5 minutes refuse the logins of everybody until they are
over. To count per browser instead, set `CERBRIX_AUTH__CLIENT_ADDRESS_HEADER=X-Forwarded-For`,
but only when every request reaches the server through the proxy and the proxy appends the
client address to that header or sets it (nginx: `proxy_set_header X-Forwarded-For
$proxy_add_x_forwarded_for;`, Caddy does it by default). The server uses the right-most address
in the header, the one the proxy added; a client that can reach the server directly could
write any address there itself. When the header is missing or holds no address, the address of
the connection counts.

## 4. Create a host and an enrolment key

A host must be registered before its agent can enrol, and enrolment needs a key. The
key is printed once and stored only as a hash. With [GitOps](#configuration-from-git) on, you
register the host by adding a file to the repository instead of `admin host add`; the key
command is the same.

```sh
docker compose exec server cerbrix-server admin host add web-01
docker compose exec server cerbrix-server admin key create --description "web-01" --expires-in 24h
```

The `key:` line of the second command is the enrolment key; you need it in the agent
configuration, see [Agent installation](#agent-installation). Further commands:
`admin key revoke <id>`, `admin host release <name>` (lets a host enrol again) and
`admin unknown-agents` (agents that tried to enrol under an unknown name).

## Configuration from Git

Cerbrix can read its whole monitoring configuration (hosts, items, triggers, templates,
notifications, dashboards, displays) from a Git repository; see
[The configuration repository](configuration-repository.md) for what the repository contains.
This is one switch for the whole installation: with GitOps on, hosts are managed only through
the repository, and `admin host add` fails with `hosts are managed by Git`.

> **Before you enable it:** when the first sync succeeds, every host that is not in the
> repository is deactivated, including hosts created earlier with `admin host add`. Their
> configuration is dropped and their agent credentials are deleted, so those agents are rejected
> until the host has a file in the repository and the agent enrols again. Put your hosts in the
> repository first and check it with `cerbrix-server check-config`.

The settings go into `deploy/server.env` (a commented block is in the example file):

| Variable | Default | Meaning |
| --- | --- | --- |
| `CERBRIX_GITOPS__ENABLED` | `false` | `true` turns GitOps on. |
| `CERBRIX_GITOPS__URL` | none | The repository. Required when enabled. `https://...`, `ssh://...`, `user@host:path`, `file://...` or an absolute local path. `http://` is rejected. |
| `CERBRIX_GITOPS__BRANCH` | `main` | The branch to read. |
| `CERBRIX_GITOPS__INTERVAL_SECS` | `60` | Seconds between checks, 5 to 3600. |
| `CERBRIX_GITOPS__TOKEN` | none | Access token for an `https://` URL. Not allowed together with an SSH key. |
| `CERBRIX_GITOPS__SSH_KEY_FILE` | none | Absolute path of a private key inside the container, for an `ssh://` or `user@host:path` URL. |
| `CERBRIX_GITOPS__KNOWN_HOSTS_FILE` | none | Absolute path of a `known_hosts` file inside the container. |
| `CERBRIX_GITOPS__PROXY` | none | Proxy for an `https://` URL: `http://`, `https://`, `socks5://` or `socks5h://`, optionally with `user:password@`. See [Behind a proxy or with a private CA](#behind-a-proxy-or-with-a-private-ca). |
| `CERBRIX_GITOPS__CA_FILE` | none | Absolute path inside the container of a PEM file with the certificate authorities to trust for an `https://` URL. It replaces the default bundle. |
| `CERBRIX_GITOPS__CACHE_DIR` | `/var/lib/cerbrix/git` | Where the server keeps its checkout. The Compose file mounts a volume here. |
| `CERBRIX_GITOPS__MAX_HOST_REMOVAL_PERCENT` | `50` | Mass-removal guard, see [the repository guide](configuration-repository.md#mass-removal-guard). |

An invalid value stops the server at start with a message that does not contain the URL or the
token. The server runs `git` itself (the image contains `git` and `openssh-client`) and fetches
only the newest commit of the branch. Only one server instance should run the sync.

### With a token (HTTPS)

The server sends the token with HTTP basic authentication, using the fixed user name
`x-access-token` and the token as the password. Use a token type that accepts any user name:
a GitHub fine-grained personal access token with read-only "Contents" access (or a classic one),
or a GitLab personal or project access token with the `read_repository` scope. Tokens that
require a specific user name, such as GitLab deploy tokens, do not work with this setting; use
[SSH with a deploy key](#with-an-ssh-key) for those. Fetching over HTTPS with a token has not
been tested end to end. Then set:

```
CERBRIX_GITOPS__ENABLED=true
CERBRIX_GITOPS__URL=https://github.com/your-org/monitoring-config.git
CERBRIX_GITOPS__TOKEN=the-token
```

The URL must not contain a user name or password; the token has its own setting. The token is
never written to the log or the database. `deploy/server.env` is not committed, keep it private.
For a public repository, leave `CERBRIX_GITOPS__TOKEN` out: an empty value is rejected and the server does not start.

### With an SSH key

Use a deploy key that can only read the repository. Put the private key and a `known_hosts`
file in a directory such as `./git-ssh` next to `docker-compose.yml`. The container user
(uid 10001) must be able to read them:

```sh
ssh-keygen -t ed25519 -N "" -f git-ssh/id_ed25519 -C cerbrix
ssh-keyscan github.com > git-ssh/known_hosts
chmod 644 git-ssh/known_hosts
sudo chown 10001 git-ssh/id_ed25519 && chmod 400 git-ssh/id_ed25519
```

Check the `known_hosts` entries against the fingerprints your Git provider publishes before you
trust them. Then add the deploy key's public part (`id_ed25519.pub`) to the repository, mount the
directory read-only by adding this line to the `volumes` of the `server` service in
`docker-compose.yml`:

```yaml
      - ./git-ssh:/etc/cerbrix/git:ro
```

and set:

```
CERBRIX_GITOPS__ENABLED=true
CERBRIX_GITOPS__URL=git@github.com:your-org/monitoring-config.git
CERBRIX_GITOPS__SSH_KEY_FILE=/etc/cerbrix/git/id_ed25519
CERBRIX_GITOPS__KNOWN_HOSTS_FILE=/etc/cerbrix/git/known_hosts
```

**Host key handling.** With `KNOWN_HOSTS_FILE` set, the server connects only to hosts listed in
that file (strict checking). Without it, the server trusts the host key it sees on the first
connection and remembers it in the cache directory (trust on first use); use the file for any
installation that matters.

Apply the settings with `docker compose up -d`.

### Behind a proxy or with a private CA

The server runs `git` with a cleared environment, so `HTTPS_PROXY`, `NO_PROXY`, `SSL_CERT_FILE`
and `GIT_SSL_CAINFO` never reach it. Use two settings instead. They apply to `https://`
repositories only; with another URL the server does not start. SSH repositories are not
affected by them: SSH connections do not go through the proxy.

- `CERBRIX_GITOPS__PROXY` is the proxy for every fetch (`NO_PROXY` has no counterpart: set
  the proxy only when the repository is reached through it). `http://` and `https://` name an
  HTTP proxy, `socks5://` a SOCKS proxy that receives an address, `socks5h://` one that
  resolves the host name itself. Credentials go into the URL, percent-encoded where needed
  (`http://user:p%40ss@proxy.example.com:3128`). The password is a secret like the token: it
  is masked where the settings are printed and removed from error messages. Keep `deploy/server.env` private.
- `CERBRIX_GITOPS__CA_FILE` is a PEM file with the certificate authorities git trusts for the
  fetch. It replaces git's default bundle instead of adding to it, so it must contain the
  authority that signed the Git server's certificate (behind a proxy that inspects TLS traffic,
  the authority the proxy signs with).

Mount the file read-only, as with the SSH key (for example `./git-ca:/etc/cerbrix/git-ca:ro`
in the `volumes` of the `server` service), and set:

```
CERBRIX_GITOPS__ENABLED=true
CERBRIX_GITOPS__URL=https://git.internal.example.com/ops/monitoring-config.git
CERBRIX_GITOPS__TOKEN=the-token
CERBRIX_GITOPS__PROXY=http://proxy.example.com:3128
CERBRIX_GITOPS__CA_FILE=/etc/cerbrix/git-ca/ca.pem
```

### Checking and operating it

```sh
docker compose exec server cerbrix-server admin sync status
docker compose exec server cerbrix-server admin sync history --limit 5
docker compose exec server cerbrix-server admin host show web-01
docker compose exec server cerbrix-server check-config /path/to/checkout
```

`sync status` shows `in sync`, `out of sync` (with the file and reason of every problem) or
`never synced`; see [Out Of Sync](configuration-repository.md#out-of-sync). Example:

```
in sync
active commit: ff92a585d63b9b3491fd8ee9c28f0d087ceaa5f6
last successful sync: 2026-11-15T00:56:48.998Z
last attempt: 2026-11-15T00:56:48.998Z
```

`host show` prints a host's file, groups and its merged items and triggers. `host release`,
`key create`, `key revoke` and `unknown-agents` work as before. Note that the commands also print
log lines to the error output; redirect it (`2>/dev/null`) in scripts.

The server keeps a checkout and a file `gitconfig` in the cache directory. Both are managed by
the server and need no editing; if the checkout is damaged the server rebuilds it.

### Limits to know

- The server runs `git` with a cleared environment: proxy and CA variables such as
  `HTTPS_PROXY` or `SSL_CERT_FILE` are not passed on. Use `CERBRIX_GITOPS__PROXY` and
  `CERBRIX_GITOPS__CA_FILE`, see [Behind a proxy or with a private CA](#behind-a-proxy-or-with-a-private-ca).
- HTTP redirects are not followed, so the token is never sent to another address. The
  configured URL must be the final one: after a repository was renamed or moved, set the new
  URL instead of relying on the provider's redirect.
- Each `git` call is stopped after 60 seconds.
- Symbolic links and submodules in the repository are rejected.
- Fetching with a token over HTTPS and with an SSH key is described here from the code; the
  automated end-to-end check (`scripts/e2e.sh`) uses a local repository.

### Agents

An agent no longer decides what it measures, whether GitOps is on or off. It fetches its
configuration from the server at start and every 60 seconds, keeps the last one in its state
directory (so it continues if the server is unreachable) and measures nothing until it has
received one. With GitOps on, the configuration comes from the host's file in the repository; a
host created with `admin host add` while GitOps is off gets all built-in items at their default
intervals. The `[intervals]` section of `agent.toml` is gone: an agent file that still has it is
rejected, so remove the section when you upgrade an existing agent.

**Upgrading from M1** (the release without configuration from Git): upgrade the server first, then the agents, and remove `[intervals]` from
each `agent.toml` as part of the agent upgrade. A new agent asks the server for its
configuration; an M1 server does not have that endpoint, so a new agent without a cached
configuration measures nothing until its server is upgraded. An M1 agent keeps working against a
new server with its local intervals until it is upgraded.

## E-mail notifications

The server sends an e-mail when a problem starts and another when it ends (see
[Alerting](alerting.md)). E-mail is off until you set the address of a mail server in
`deploy/server.env`; the example file has the settings, commented out:

| Variable | Default | Meaning |
| --- | --- | --- |
| `CERBRIX_SMTP__HOST` | none (e-mail off) | Name of the mail server. |
| `CERBRIX_SMTP__PORT` | `587` | 587 for `starttls`, 465 for `tls`. |
| `CERBRIX_SMTP__ENCRYPTION` | `starttls` | `starttls`, `tls` or `none` (plain text, for a trusted local relay only; not allowed together with a user name and password). |
| `CERBRIX_SMTP__FROM` | none | The sender, for example `Cerbrix <cerbrix@example.com>`. Required when a host is set. |
| `CERBRIX_SMTP__USERNAME`, `CERBRIX_SMTP__PASSWORD` | none | Login; set both or neither. |
| `CERBRIX_SMTP__TIMEOUT_SECS` | `15` | Seconds allowed for one message, 1 to 120. |

Apply the settings with `docker compose up -d` and send a test message:

```sh
docker compose exec server cerbrix-server admin notify test you@example.com
```

Only the public certificate authorities are trusted for `starttls` and `tls`; a mail relay with a
certificate from an internal CA cannot be used with them yet. Who receives which problem is set by
the notification files in the configuration repository, see
[Who is told](alerting.md#who-is-told-notification-rules). Two further settings, the age limit
for values and the interval of the no-data check, are in
[Settings for alerting](alerting.md#settings-for-alerting).

**Upgrading from M2** (configuration from Git, no alerting): nothing to do. The server applies its
database migration (new tables for problems and the e-mail queue) when it starts. Without SMTP
settings it records problems but sends no e-mail; hosts with no-data triggers count silence from
the start of the server, so the upgrade does not report any host as silent. If a host has an
item-level no-data trigger without a host-level one, the configuration is now refused (Out Of
Sync) until you add the host-level trigger; see [Alerting](alerting.md#when-data-stops-arriving).

## Retention

Two settings decide how long data is kept:

| Variable | Default | Meaning |
| --- | --- | --- |
| `CERBRIX_METRICS_RETENTION` | `12` | How long VictoriaMetrics keeps metric values. A number without a unit means months; `90d` (days) or `2y` (years) are possible too. A Compose variable: set it in the shell environment or in the `.env` file next to `docker-compose.yml`, not in `deploy/server.env`. |
| `CERBRIX_RETENTION__CLOSED_PROBLEMS_DAYS` | `365` | Problems resolved longer ago than this many days are deleted, together with their notification records (the queued and sent e-mails). `0` keeps them for ever, otherwise 1 to 3650. Set in `deploy/server.env`. |

VictoriaMetrics removes values older than its retention by itself; the server never deletes
metric values. Shortening `CERBRIX_METRICS_RETENTION` and restarting the stack
(`docker compose up -d`) deletes the values that are older than the new period, and they cannot
be brought back. Lengthening it keeps values from then on, it does not bring back values already
removed.

The server deletes expired problems once an hour, the first time right after it starts, in batches of
5000 problems so that no single step holds the table for long. Open problems are never deleted, however long ago they started: only the time a problem
was resolved counts. The sync history is kept; retention does not touch it. An invalid value of
`CERBRIX_RETENTION__CLOSED_PROBLEMS_DAYS` stops the server at start.

## Making the server reachable for agents

Agents require `https` by default, so before agents on other machines can connect you need
TLS. There are two supported ways.

> **Warning:** `CERBRIX_SERVER_BIND=0.0.0.0` together with
> `CERBRIX_SERVER__INSECURE_HTTP=true` sends enrolment keys and agent credentials
> unencrypted over the network. Use that combination only on isolated test networks.

The compose variables `CERBRIX_SERVER_BIND` (host address of the server port, default
`127.0.0.1`) and `CERBRIX_SERVER_PORT` (default `8443`) are read by Docker Compose from the
shell environment or from a `.env` file next to `docker-compose.yml`, for example
`CERBRIX_SERVER_BIND=0.0.0.0`. They are not set in `deploy/server.env`, which configures
the containers only.

**(a) The server terminates TLS.** Put the certificate chain and private key in a directory
such as `./tls` next to `docker-compose.yml`, uncomment the `volumes` entry of the `server`
service, remove `CERBRIX_SERVER__INSECURE_HTTP` from `deploy/server.env` and set:

```
CERBRIX_SERVER__TLS_CERT=/etc/cerbrix/tls/fullchain.pem
CERBRIX_SERVER__TLS_KEY=/etc/cerbrix/tls/privkey.pem
```

The files must be readable by the container user (uid 10001). Then set
`CERBRIX_SERVER_BIND=0.0.0.0` in the `.env` file and run `docker compose up -d`. For a
certificate from a private CA, agents need the CA certificate through their `ca_file`
setting.

**(b) A reverse proxy in front.** Keep plain HTTP and the default bind address
`127.0.0.1`. Run a TLS-terminating proxy (nginx, Caddy, Traefik, ...) on the same host that
listens on port 443 and forwards to `127.0.0.1:8443`. Agents then use
`https://your-server-name` as `server_url`. The same proxy can serve the web UI and the
[API](api.md); it must pass the `Host` header unchanged and you must set
`CERBRIX_AUTH__SECURE_COOKIE=true`, see [Behind a reverse proxy](#behind-a-reverse-proxy).

## Building the server from source

The Docker image builds everything. To build the server binary yourself with `cargo build --release`,
the web UI must be built first, because the binary embeds it: install Node 22 or newer and run
`npm ci && npm run build` in `ui/` before building the server. Without `ui/dist` the build still
works, but the server then serves a placeholder page instead of the UI (a release build prints a
warning, and the server logs one when it starts); `CERBRIX_REQUIRE_UI=1` (or `true`) makes a
missing UI fail the build.

## Agent installation

The agent runs on every monitored Linux host (x86_64 or aarch64, with systemd) as a
systemd service under its own `cerbrix` user. The install script downloads the release
binary, verifies its SHA-256 checksum, creates the user and the configuration and starts
the service. You need the host name and the enrolment key from
[step 4](#4-create-a-host-and-an-enrolment-key), and the server must be reachable over
`https` ([Making the server reachable for agents](#making-the-server-reachable-for-agents)).

Download the script, read it, then run it as root. Pass the key through the environment
variable `CERBRIX_ENROLLMENT_KEY`: unlike `--enrollment-key` it does not end up in the
shell history or in the process list.

```sh
curl -fsSLO https://github.com/TrisMarzec/Cerbrix/releases/latest/download/install-agent.sh
less install-agent.sh
sh install-agent.sh --dry-run --server-url https://your-server-name --host web-01
printf 'Enrolment key: '; stty -echo; read -r CERBRIX_ENROLLMENT_KEY; stty echo; echo
export CERBRIX_ENROLLMENT_KEY
sudo --preserve-env=CERBRIX_ENROLLMENT_KEY sh install-agent.sh \
    --server-url https://your-server-name --host web-01
```

The `--dry-run` line prints every step as `would: ...` without changing anything (it does
not need root, and it works before you have entered the key). Options:

| Option | Meaning |
| --- | --- |
| `--server-url URL` | `https` URL of the server (required on first install) |
| `--host NAME` | the host name registered with `admin host add` (required on first install) |
| `--enrollment-key KEY` | the enrolment key; prefer `CERBRIX_ENROLLMENT_KEY` |
| `--ca-file PATH` | PEM certificate of the CA that signed the server certificate; copied to `/etc/cerbrix/ca.pem` |
| `--version X.Y.Z` | agent version, default: the latest release |
| `--dry-run` | show the steps only |

The script installs `/usr/local/bin/cerbrix-agent`, the unit
`/etc/systemd/system/cerbrix-agent.service` and the configuration
`/etc/cerbrix/agent.toml` (mode 0640, owner `root:cerbrix`). After the first successful
enrolment the agent keeps its own credential in `/var/lib/cerbrix-agent`, and the
enrolment key stays in the configuration for the case that the server rejects the stored
credential. Which items the agent collects, and how often, comes from the server (Git
configuration); the local settings (excluded file systems, mounts and interfaces) are keys
of `agent.toml`; edit the file and run `sudo systemctl restart cerbrix-agent`. To
change the unit, use `sudo systemctl edit cerbrix-agent` instead of editing the unit file.

**Check it and read the log:**

```sh
systemctl status cerbrix-agent
journalctl -u cerbrix-agent -f
```

**Upgrade:** run the script again, with `--version` for a specific release. It replaces
the binary and the unit file, restarts the service and keeps an existing
`/etc/cerbrix/agent.toml` unchanged, so `--server-url`, `--host` and the key are not needed
(they are ignored if given).

**Uninstall:**

```sh
sudo systemctl disable --now cerbrix-agent
sudo rm /etc/systemd/system/cerbrix-agent.service /usr/local/bin/cerbrix-agent
sudo systemctl daemon-reload
sudo rm -r /etc/cerbrix /var/lib/cerbrix-agent
sudo userdel cerbrix
```

Then release the host on the server (`admin host release <name>`) if it will enrol again.

## Verifying an installation

`scripts/e2e.sh` starts a separate throw-away stack with its configuration coming from a local
Git repository, enrols a locally built agent and checks that its values arrive in
VictoriaMetrics. It then pushes a broken host file, checks that the installation reports
`out of sync` while values keep arriving, and that reverting the commit makes it `in sync`
again. It also checks alerting against a real mail server (Mailpit): a problem opens and is mailed, resolves and is mailed, an acknowledgement shows in the problem list, and a stopped agent is reported. Finally it walks through the [API](api.md) with `curl`: login, status, hosts, a series, problems with an acknowledgement, a dashboard, a wall display with its token, and the access rules. It does not test TLS or a login at the mail server; use `admin notify test` for that. It needs Docker, curl 7.55 or newer, awk, git and a Rust toolchain; CI runs it on every push.

## Troubleshooting

**The agent logs `already_bound`.** The host name is already tied to an earlier credential,
usually after a failed first enrolment or a reinstall. Run
`cerbrix-server admin host release <name>` on the server, then restart the agent.

**`docker compose up` fails with postgres unhealthy.** Check `docker compose logs postgres`.
The database password in `deploy/server.env` is probably empty.
