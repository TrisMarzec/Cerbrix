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
key is printed once and stored only as a hash:

```sh
docker compose exec server cerbrix-server admin host add web-01
docker compose exec server cerbrix-server admin key create --description "web-01" --expires-in 24h
```

The `key:` line of the second command is the enrolment key; you need it in the agent
configuration, see [Agent installation](#agent-installation). Further commands:
`admin key revoke <id>`, `admin host release <name>` (lets a host enrol again) and
`admin unknown-agents` (agents that tried to enrol under an unknown name).

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
credential. Further settings (collection intervals, excluded mounts and interfaces) are
keys of `agent.toml`; edit the file and run `sudo systemctl restart cerbrix-agent`. To
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

`scripts/e2e.sh` starts a separate throw-away stack, enrols a locally built agent and checks
that its values arrive in VictoriaMetrics. It needs Docker, curl and a Rust toolchain; CI runs
it on every push.

## Troubleshooting

**The agent logs `already_bound`.** The host name is already tied to an earlier credential,
usually after a failed first enrolment or a reinstall. Run
`cerbrix-server admin host release <name>` on the server, then restart the agent.

**`docker compose up` fails with postgres unhealthy.** Check `docker compose logs postgres`.
The database password in `deploy/server.env` is probably empty.
