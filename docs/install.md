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
`postgres`, `victoriametrics` (12 months retention) and `server`. The server applies its
database migrations on start. By default the server is published on `127.0.0.1:8443` and
VictoriaMetrics on `127.0.0.1:8428` only; PostgreSQL is not published at all. Check it:

```sh
curl http://127.0.0.1:8443/healthz
```

## 3. Create a host and an enrolment key

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

- The server runs `git` with a cleared environment. Proxy settings (`HTTPS_PROXY`) and custom CA
  certificate variables are not passed on, so the repository must be reachable directly and with
  a certificate that the system trusts. This is a known limitation.
- Each `git` call is stopped after 60 seconds.
- Symbolic links and submodules in the repository are rejected.
- Fetching with a token over HTTPS and with an SSH key is described here from the code; the
  automated end-to-end check (`scripts/e2e.sh`) uses a local repository.

### Agents

With GitOps on, an agent no longer decides what it measures. It fetches its configuration from
the server at start and every 60 seconds, keeps the last one in its state directory (so it
continues if the server is unreachable) and measures nothing until it has received one. A host
created with `admin host add` while GitOps is off gets all built-in items at their default
intervals. The `[intervals]` section of `agent.toml` is gone: an agent file that still has it is
rejected, so remove the section when you upgrade an existing agent.

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
`https://your-server-name` as `server_url`.

## Agent installation

The agent runs on every monitored Linux host (x86_64 or aarch64, with systemd) as a
systemd service under its own `cerbrix` user. The install script downloads the release
binary, verifies its SHA-256 checksum, creates the user and the configuration and starts
the service. You need the host name and the enrolment key from
[step 3](#3-create-a-host-and-an-enrolment-key), and the server must be reachable over
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
again. It needs Docker, curl, git and a Rust toolchain; CI runs it on every push.

## Troubleshooting

**The agent logs `already_bound`.** The host name is already tied to an earlier credential,
usually after a failed first enrolment or a reinstall. Run
`cerbrix-server admin host release <name>` on the server, then restart the agent.

**`docker compose up` fails with postgres unhealthy.** Check `docker compose logs postgres`.
The database password in `deploy/server.env` is probably empty.
