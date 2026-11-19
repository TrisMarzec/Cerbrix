# Contributing to Cerbrix

## Prerequisites

- Rust (stable); `rust-toolchain.toml` installs `rustfmt` and `clippy` automatically via rustup
- Node 22 or newer with npm, for work on the web UI in `ui/` (the server alone does not need it)
- Docker with Docker Compose, for the development databases
- bash and curl, for the end-to-end check `scripts/e2e.sh` (Git Bash on Windows)

## Checks

These four commands must pass before every commit and pull request:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --locked
cargo test --workspace
```

## Web UI

The web UI is a Preact (version 11) and TypeScript application in `ui/`. Run these in `ui/`:

```sh
npm ci            # install the locked dependencies
npm run check     # type check and tests (run before every commit)
npm run build     # type check, production build, licence file, size budget; then ui/dist is replaced
```

`npm run dev` starts the Vite dev server with hot reload. It proxies `/api` and `/healthz` to
`http://127.0.0.1:8443`, so run a Cerbrix server beside it that listens there (with
`insecure_http` set, because the proxy speaks plain HTTP) and open the address Vite prints.

`npm run build` builds into `ui/dist.tmp` (git-ignored), runs the licence and size checks on it
and only when both passed replaces `ui/dist` with it and re-creates `ui/dist/.gitkeep`. If a check
fails, `ui/dist` stays as it was, so the next `cargo build` cannot embed a UI that failed its
checks. The size check also rejects an inline script, a style element or attribute and an inline
event-handler attribute in `index.html`, which the content security policy would block.

The server embeds `ui/dist` when it is compiled. A plain `cargo build` on a checkout without
`ui/dist` works without Node and embeds a placeholder page that says how to build the UI; run
`npm run build` and build the server again to embed the real one. `ui/dist` is ignored by git
except for the empty `ui/dist/.gitkeep`, which keeps the directory (the one path the build script
watches) in every checkout. Two environment variables steer `crates/cerbrix-server/build.rs`:

- `CERBRIX_UI_DIST` makes the server embed the UI from another directory (the build prints a
  warning when it is set). A relative path is taken relative to the workspace root, not to the
  directory you run cargo in.
- `CERBRIX_REQUIRE_UI` set to `1` or `true` makes a missing UI fail the build (the Docker image
  sets it, so a release never ships the placeholder). Any other non-empty value fails the build,
  so a typo is not silently ignored.

A release build (`--release`) that embeds the placeholder prints a cargo warning, and a server
that embeds it logs a warning when it starts.

## Development databases

Start PostgreSQL (port 5432) and VictoriaMetrics (port 8428):

```sh
docker compose -f docker-compose.dev.yml up -d
```

Database tests need this environment variable:

```sh
export DATABASE_URL=postgres://cerbrix:cerbrix@localhost:5432/cerbrix
```

## End-to-end check

`scripts/e2e.sh` builds the server image, starts its own Compose stack (project
`cerbrix-e2e`, host ports 18443 and 18428, configurable with `CERBRIX_SERVER_PORT` and
`CERBRIX_VM_PORT`), runs an agent against it and checks that values arrive. It does not touch
the development databases and removes its stack when done.

At its end the script runs the browser smoke run (below) in the Playwright Docker image, on the
network of the stack. The first run pulls that image, which is large; `CERBRIX_E2E_SKIP_UI=1`
skips the smoke run. `CERBRIX_UI_SMOKE_FAIL=1` makes it fail in its first step on purpose, and
`CERBRIX_UI_SMOKE_FAIL=login` makes the login fail after the password was typed, to see how a
failure is reported (the output is printed with tokens and the exact admin password masked, and
the stack is still removed). The project name (`CERBRIX_E2E_PROJECT`) is lower-cased, because
Compose lower-cases it in the name of the network that the smoke run joins.

## Browser smoke run

`ui/e2e/smoke.spec.ts` is one Playwright test (Chromium) that logs in, visits every page,
creates a display token, opens the dashboard `e2e` on the screen, starts the wall display with
the token in a second browser context without a session (where it shows the alarm), then
acknowledges the problem and logs out. It fails on any console error,
failed request to the server, status 500 or higher and content security policy violation. It
needs a running server in the state `scripts/e2e.sh` leaves before this step:

- an admin user,
- the dashboard `e2e` and the display `e2e-wall` (shown as "E2E wall"), which has no token
  because the API walk-through of the script created and revoked its token,
- the open, unacknowledged problem "Agent silent" of the host `e2e-host`, because the script
  stopped the agent,
- the configuration in sync (see `CERBRIX_UI_SMOKE_EXPECT_SYNC` below).

The run fails on the first unexpected 401 answer: a load of a page without a login is allowed
one 401 for `GET /session`, a wrong password one more, the wall display opened without a token
one for `GET /display/e2e-wall`, and the wrong token entered on it one more for the same route;
the run fails when an allowed one does not happen. The spec never passes the password or the
display token to Playwright's `fill`, whose error log echoes the value; it sets the field's value
with `evaluate`, which is not logged. `npm run check`
does not run the smoke run. By hand, in `ui/`:

```sh
npx playwright install chromium     # once
export CERBRIX_UI_URL=http://127.0.0.1:18443
export CERBRIX_UI_USER=e2e-admin
export CERBRIX_UI_PASSWORD_FILE=/path/to/file-with-the-password
npm run smoke
```

To run it by hand against a stack kept with `CERBRIX_E2E_KEEP_STACK=1` (the project is
`cerbrix-e2e` unless you named it otherwise; its server container is `cerbrix-e2e-server-1`),
create a user inside the server container, write the password it prints into a file and point
the three variables at the stack:

```sh
docker exec cerbrix-e2e-server-1 cerbrix-server admin user create smoke --role admin
# put the printed password (the line "password: ...") into a file, for example with: read -rs p; printf '%s' "$p" > pw
export CERBRIX_UI_URL=http://127.0.0.1:18443
export CERBRIX_UI_USER=smoke
export CERBRIX_UI_PASSWORD_FILE=$PWD/pw
export CERBRIX_UI_SMOKE_EXPECT_SYNC=out_of_sync
npm run smoke
```

The kept stack is out of sync: the script removes the configuration repository (its work
directory) when it ends, so the server cannot fetch it any more. That is why
`CERBRIX_UI_SMOKE_EXPECT_SYNC=out_of_sync` is needed there; the default is `in_sync`. The run is
not repeatable: it acknowledges the problem and creates a display token, so a second run on the
same stack does not find the problem unacknowledged.

The smoke run starts Chromium with a flag that treats the test origin (plain HTTP under a name
other than `localhost`) as a secure context, so that the browser does not log a message about
the server's security headers. It therefore does not cover what the UI does over plain HTTP
under such a name: there the session cookie has no `Secure` attribute, the browser gives the page
no clipboard (every Copy button asks to select the text and press Ctrl+C instead), and the
console shows a message that the `Cross-Origin-Opener-Policy` header is ignored. See
[The web UI](docs/web-ui.md#plain-http).

The automated check runs in Chromium only.
