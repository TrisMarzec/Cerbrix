# Contributing to Cerbrix

## Prerequisites

- Rust (stable); `rust-toolchain.toml` installs `rustfmt` and `clippy` automatically via rustup
- Docker with Docker Compose, for the development databases

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
