# syntax=docker/dockerfile:1

# UI stage: builds the web UI (ui/dist) that the server embeds. Only ui/ is copied, so a
# change elsewhere does not rebuild it.
FROM node:22-slim AS ui
WORKDIR /ui
COPY ui/ ./
RUN npm ci && npm run build

# Build stage: the official Rust image tracks the stable channel, like rust-toolchain.toml.
FROM rust:1 AS build
WORKDIR /src
COPY . .
# The server embeds the built UI at compile time (build.rs reads ui/dist).
COPY --from=ui /ui/dist ui/dist
# The cache mounts keep the cargo registry and the target directory between builds, so
# only changed crates are recompiled. The binary is copied out inside the same RUN
# because the target directory is not part of the image layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p cerbrix-server \
    && cp target/release/cerbrix-server /usr/local/bin/cerbrix-server

# Runtime stage: the server binary and the git and ssh clients it needs to fetch the
# configuration repository, running as an unprivileged user.
FROM debian:stable-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin cerbrix \
    && install -d -o cerbrix -g cerbrix -m 0700 /var/lib/cerbrix/git
COPY --from=build /usr/local/bin/cerbrix-server /usr/local/bin/cerbrix-server
USER cerbrix
EXPOSE 8443
# Plain HTTP when insecure_http is set, otherwise TLS (the certificate is not checked
# because the probe talks to localhost).
HEALTHCHECK --interval=10s --timeout=5s --start-period=10s --retries=5 \
    CMD curl -fsS http://127.0.0.1:8443/healthz || curl -fsSk https://127.0.0.1:8443/healthz || exit 1
ENTRYPOINT ["cerbrix-server"]
CMD ["serve"]
