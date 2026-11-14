# Installing the Cerbrix server

This guide installs the Cerbrix server together with PostgreSQL and VictoriaMetrics
using Docker Compose.

## Prerequisites

- A Linux host (or any host that runs Docker) with Docker and the Docker Compose plugin
- A DNS name for the server that your monitored hosts can reach
- A TLS certificate for that name, or a reverse proxy that provides one (see [TLS](#tls))

## 1. Get the sources and configure

```sh
git clone https://github.com/TrisMarzec/Cerbrix.git
cd Cerbrix
cp deploy/server.env.example deploy/server.env
```

Edit `deploy/server.env`:

- Replace the placeholder password `change-me-before-first-start` in **both**
  `POSTGRES_PASSWORD` and `CERBRIX_DATABASE__URL`. Both must carry the same password.
- Decide how TLS is handled (below).

`deploy/server.env` is excluded from Git. Keep it private; it holds the database
password.

## 2. Start the stack

```sh
docker compose up -d --build
docker compose ps
```

The first start builds the server image, which takes several minutes. The stack consists of
`postgres`, `victoriametrics` (12 months retention) and `server` (port 8443). The server
applies its database migrations on start. Check it with:

```sh
curl http://127.0.0.1:8443/healthz     # or https://... with -k when TLS is enabled
```

VictoriaMetrics is published on `127.0.0.1:8428` only, because it has no authentication.
PostgreSQL is not published at all. Set `CERBRIX_SERVER_PORT` or `CERBRIX_VM_PORT` in the
environment to change the host ports.

## 3. Create a host and an enrolment key

A host must be registered before its agent can enrol, and enrolment needs a key. The
key is printed once and stored only as a hash:

```sh
docker compose exec server cerbrix-server admin host add web-01
docker compose exec server cerbrix-server admin key create --description "web-01" --expires-in 24h
```

The `key:` line of the second command is the enrolment key; copy it into the agent
configuration (next section). Further commands: `admin key revoke <id>`,
`admin host release <name>` (lets a host enrol again) and `admin unknown-agents`.

## TLS

Agents require `https` by default, so the server needs TLS in front of it. There are two options.

**Option A: a reverse proxy in front.** Leave `CERBRIX_SERVER__INSECURE_HTTP=true`, so the
server speaks plain HTTP, and publish the port only to the proxy (for example change the
mapping in `docker-compose.yml` to `127.0.0.1:8443:8443`). The proxy (nginx, Caddy,
Traefik, ...) terminates TLS on port 443 and forwards to `127.0.0.1:8443`. Agents then use
`https://your-server-name` as `server_url`.

**Option B: the server terminates TLS.** Put the certificate chain and the private key in
a directory such as `./tls` next to `docker-compose.yml`, uncomment the `volumes` entry of the
`server` service in `docker-compose.yml`, remove `CERBRIX_SERVER__INSECURE_HTTP` from
`deploy/server.env` and set:

```
CERBRIX_SERVER__TLS_CERT=/etc/cerbrix/tls/fullchain.pem
CERBRIX_SERVER__TLS_KEY=/etc/cerbrix/tls/privkey.pem
```

The files must be readable by the container user (uid 10001). Restart with
`docker compose up -d`. For a certificate from a private CA, agents need the CA
certificate through their `ca_file` setting.

## Agent installation

Installing agents on the monitored hosts is described in the next section of this guide.

## Verifying an installation

`scripts/e2e.sh` starts a separate throw-away stack, enrols a locally built agent and checks
that its values arrive in VictoriaMetrics. It needs Docker, curl and a Rust toolchain; CI runs
it on every push.
