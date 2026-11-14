#!/usr/bin/env bash
# End-to-end check: starts the production compose stack under its own project name,
# enrols a locally built agent and waits until its values show up in VictoriaMetrics.
#
# Environment variables (defaults in parentheses):
#   CERBRIX_E2E_PROJECT      compose project name (cerbrix-e2e)
#   CERBRIX_SERVER_PORT      host port of the server (18443)
#   CERBRIX_VM_PORT          host port of VictoriaMetrics (18428)
#   CERBRIX_E2E_TIMEOUT      seconds to wait for both series (120)
#   CERBRIX_E2E_KEEP_STACK   set to 1 to leave the stack running (for debugging)
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
ROOT=$(pwd)

PROJECT=${CERBRIX_E2E_PROJECT:-cerbrix-e2e}
export CERBRIX_SERVER_PORT=${CERBRIX_SERVER_PORT:-18443}
export CERBRIX_VM_PORT=${CERBRIX_VM_PORT:-18428}
TIMEOUT=${CERBRIX_E2E_TIMEOUT:-120}
HOST_NAME=e2e-host
POLL_INTERVAL=5

WORK=$(mktemp -d)
AGENT_PID=""
export CERBRIX_ENV_FILE="$WORK/server.env"

compose() { docker compose -p "$PROJECT" "$@"; }
fail() { echo "e2e: FAIL: $*" >&2; exit 1; }
# Native path for programs that are not MSYS aware (the agent on Windows).
native_path() { if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s' "$1"; fi; }

cleanup() {
    status=$?
    set +e
    if [ -n "$AGENT_PID" ]; then
        kill "$AGENT_PID" 2>/dev/null
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            kill -0 "$AGENT_PID" 2>/dev/null || break
            sleep 1
        done
        if kill -0 "$AGENT_PID" 2>/dev/null; then
            kill -9 "$AGENT_PID" 2>/dev/null
            command -v taskkill >/dev/null 2>&1 && taskkill //F //PID "$AGENT_PID" >/dev/null 2>&1
            sleep 1
        fi
        if kill -0 "$AGENT_PID" 2>/dev/null; then
            echo "e2e: WARNING: agent process $AGENT_PID is still running" >&2
        else
            echo "e2e: agent stopped"
        fi
    fi
    if [ "$status" -ne 0 ] && [ -f "$WORK/agent.log" ]; then
        echo "e2e: last agent log lines:" >&2
        tail -n 20 "$WORK/agent.log" >&2
    fi
    if [ "$status" -ne 0 ] && [ "${CERBRIX_E2E_KEEP_STACK:-0}" != 1 ]; then
        compose logs --tail 30 server >&2 2>/dev/null
    fi
    if [ "${CERBRIX_E2E_KEEP_STACK:-0}" = 1 ]; then
        echo "e2e: leaving stack '$PROJECT' running as requested"
    else
        compose down -v --remove-orphans >/dev/null 2>&1 && echo "e2e: stack removed"
    fi
    rm -rf "$WORK"
    exit "$status"
}
trap cleanup EXIT

command -v docker >/dev/null 2>&1 || fail "docker is not installed"
command -v curl >/dev/null 2>&1 || fail "curl is not installed"
command -v cargo >/dev/null 2>&1 || export PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || fail "cargo is not installed"

echo "e2e: building the agent"
cargo build -p cerbrix-agent >&2 || fail "cargo build failed"
AGENT_BIN="$ROOT/target/debug/cerbrix-agent"
[ -x "$AGENT_BIN" ] || AGENT_BIN="$ROOT/target/debug/cerbrix-agent.exe"
[ -x "$AGENT_BIN" ] || fail "agent binary not found in target/debug"

# Throw-away server configuration with a random database password.
DB_PASSWORD=$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')
sed -e "s/change-me-before-first-start/$DB_PASSWORD/g" deploy/server.env.example > "$WORK/server.env"

echo "e2e: starting the stack '$PROJECT' (server on port $CERBRIX_SERVER_PORT, VictoriaMetrics on $CERBRIX_VM_PORT)"
compose up -d --build --wait --wait-timeout 300 >&2 || fail "the stack did not become healthy"

echo "e2e: creating host $HOST_NAME and an enrolment key"
compose exec -T server cerbrix-server admin host add "$HOST_NAME" >/dev/null \
    || fail "admin host add failed"
KEY_OUTPUT=$(compose exec -T server cerbrix-server admin key create \
    --description e2e --expires-in 1h 2>/dev/null) || fail "admin key create failed"
KEY=$(printf '%s\n' "$KEY_OUTPUT" | tr -d '\r' | sed -n 's/^key: //p')
unset KEY_OUTPUT
[ -n "$KEY" ] || fail "no enrolment key found in the output of admin key create"

mkdir "$WORK/state"
cat > "$WORK/agent.toml" <<TOML
server_url = "http://127.0.0.1:$CERBRIX_SERVER_PORT"
allow_insecure_http = true
host = "$HOST_NAME"
enrollment_key = "$KEY"
state_dir = "$(native_path "$WORK/state")"

[intervals]
cpu = "2s"
load = "2s"
memory = "2s"
uptime = "2s"
disk = "2s"
net = "2s"
TOML
unset KEY

echo "e2e: starting the agent"
"$AGENT_BIN" --config "$(native_path "$WORK/agent.toml")" >"$WORK/agent.log" 2>&1 &
AGENT_PID=$!

# Prints the number of samples of one series seen in the last 10 minutes (0 if none).
sample_count() {
    local body
    body=$(curl -fsS -G "http://127.0.0.1:$CERBRIX_VM_PORT/api/v1/query" \
        --data-urlencode "query=count_over_time($1{host=\"$HOST_NAME\"}[10m])" 2>/dev/null) || { echo 0; return; }
    printf '%s' "$body" | sed -n 's/.*"value":\[[0-9.]*,"\([0-9]*\)"\].*/\1/p' | head -n 1 | grep . || echo 0
}

# VictoriaMetrics hides the newest ~30 s of data, so poll instead of asking once.
echo "e2e: waiting up to ${TIMEOUT}s for samples"
deadline=$((SECONDS + TIMEOUT))
while :; do
    kill -0 "$AGENT_PID" 2>/dev/null || fail "the agent exited early"
    cpu=$(sample_count cerbrix_cpu_usage)
    disk=$(sample_count cerbrix_disk_used_percent)
    echo "e2e: cerbrix_cpu_usage=$cpu cerbrix_disk_used_percent=$disk"
    if [ "$cpu" -gt 0 ] && [ "$disk" -gt 0 ]; then
        echo "e2e: OK: cerbrix_cpu_usage has $cpu samples, cerbrix_disk_used_percent has $disk samples"
        exit 0
    fi
    [ "$SECONDS" -lt "$deadline" ] || fail "no samples for both series within ${TIMEOUT}s"
    sleep "$POLL_INTERVAL"
done
