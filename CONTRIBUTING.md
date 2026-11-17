# Contributing to Cerbrix

## Prerequisites

- Rust (stable); `rust-toolchain.toml` installs `rustfmt` and `clippy` automatically via rustup
- Node 22 or newer with npm, for work on the web UI in `ui/` (the server alone does not need it)
- Docker with Docker Compose, for the development databases
- bash and curl, for the end-to-end check `scripts/e2e.sh` (Git Bash on Windows)

## Checks

These three commands must pass before every commit and pull request:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --locked
cargo test --workspace
```

## Web UI

The web UI is a Preact and TypeScript application in `ui/`. Run these in `ui/`:

```sh
npm ci            # install the locked dependencies
npm run check     # type check and tests (run before every commit)
npm run build     # type check, production build in ui/dist, licence file, size budget
```

`npm run dev` starts the Vite dev server with hot reload. It proxies `/api` and `/healthz` to
`http://127.0.0.1:8443`, so run a Cerbrix server beside it that listens there (with
`insecure_http` set, because the proxy speaks plain HTTP) and open the address Vite prints.

The server embeds `ui/dist` when it is compiled. A plain `cargo build` on a checkout without
`ui/dist` works without Node and embeds a placeholder page that says how to build the UI; run
`npm run build` and build the server again to embed the real one. `ui/dist` is
ignored by git except for the empty `ui/dist/.gitkeep`, which keeps the directory (the one path
the build script watches) in every checkout; `npm run build` re-creates it. Setting
`CERBRIX_UI_DIST` to another directory makes the server embed the UI from there (the build
prints a warning when it is set), and `CERBRIX_REQUIRE_UI=1` makes a missing UI fail the build.

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
skips the smoke run. `CERBRIX_UI_SMOKE_FAIL=1` makes it fail on purpose, to see how a failure
is reported (the output is printed with tokens masked and the stack is still removed).

## Browser smoke run

`ui/e2e/smoke.spec.ts` is one Playwright test (Chromium) that logs in, visits every page,
acknowledges a problem, creates a display token and logs out. It fails on any console error,
failed request to the server, status 500 or higher and content security policy violation. It
needs a running server in the state `scripts/e2e.sh` leaves before this step: an admin user,
the dashboard `e2e`, the display `e2e-wall` and the open problem "Agent silent" of the host
`e2e-host`. `npm run check` does not run it. By hand, in `ui/`:

```sh
npx playwright install chromium     # once
export CERBRIX_UI_URL=http://127.0.0.1:18443
export CERBRIX_UI_USER=e2e-admin
export CERBRIX_UI_PASSWORD_FILE=/path/to/file-with-the-password
npm run smoke
```

Against a stack that was kept with `CERBRIX_E2E_KEEP_STACK=1` the run is not repeatable: it
acknowledges the problem, so the second run does not find it unacknowledged.
