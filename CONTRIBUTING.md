# Contributing to Cerbrix

## Prerequisites

- Rust (stable); `rust-toolchain.toml` installs `rustfmt` and `clippy` automatically via rustup
- Docker with Docker Compose, for the development databases
- bash and curl, for the end-to-end check `scripts/e2e.sh` (Git Bash on Windows)

## Checks

These three commands must pass before every commit and pull request:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

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
