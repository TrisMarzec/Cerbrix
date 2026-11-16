#!/usr/bin/env bash
# End-to-end check: starts the production compose stack under its own project name with
# the configuration coming from a Git repository, enrols a locally built agent and waits
# until its values show up in VictoriaMetrics.
#
# The repository is a bare repository in a temporary directory, mounted read-only into the
# server container. The script then pushes a broken host file and checks that the
# installation reports "out of sync" (naming the file) while values keep arriving, and that
# reverting the commit brings it back "in sync".
#
# Alerting is checked against a real mail server (Mailpit, plain SMTP on the compose network):
# a problem opens and its e-mail arrives, the problem resolves and the second e-mail arrives,
# an acknowledgement shows in the problem list, and after the agent is stopped the no-data
# problem opens and is mailed. No message may arrive twice. TLS and authentication of a real
# SMTP server are not covered; use `admin notify test` for that.
#
# At the end the JSON API of the web UI is walked through with curl: an admin is created with
# the admin command and logs in, the status, the hosts, a series, the problems (one is
# acknowledged through the API), a dashboard and a wall display with its token are read, and
# the access rules are checked (no cookie is 401, a viewer cannot create users, a display
# token opens only its own display). The JSON is checked with grep on the compact answers
# (no jq). On a failure the last HTTP status and body are printed with tokens masked; the
# admin password and the display token are never printed.
#
# Environment variables (defaults in parentheses):
#   CERBRIX_E2E_PROJECT       compose project name (cerbrix-e2e)
#   CERBRIX_SERVER_PORT       host port of the server (18443)
#   CERBRIX_VM_PORT           host port of VictoriaMetrics (18428)
#   CERBRIX_E2E_TIMEOUT       seconds to wait for both series (120)
#   CERBRIX_E2E_SYNC_TIMEOUT  seconds to wait for a sync status (90)
#   CERBRIX_MAILPIT_PORT      host port of the Mailpit web interface and API (18025)
#   CERBRIX_E2E_ALERT_TIMEOUT seconds to wait for a problem or an e-mail (120)
#   CERBRIX_E2E_NODATA_TIMEOUT seconds to wait for the no-data problem after the agent stopped (240)
#   CERBRIX_E2E_KEEP_STACK    set to 1 to leave the stack running (for debugging)
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
ROOT=$(pwd)

PROJECT=${CERBRIX_E2E_PROJECT:-cerbrix-e2e}
export CERBRIX_SERVER_PORT=${CERBRIX_SERVER_PORT:-18443}
export CERBRIX_VM_PORT=${CERBRIX_VM_PORT:-18428}
TIMEOUT=${CERBRIX_E2E_TIMEOUT:-120}
SYNC_TIMEOUT=${CERBRIX_E2E_SYNC_TIMEOUT:-90}
MAILPIT_PORT=${CERBRIX_MAILPIT_PORT:-18025}
ALERT_TIMEOUT=${CERBRIX_E2E_ALERT_TIMEOUT:-120}
NODATA_TIMEOUT=${CERBRIX_E2E_NODATA_TIMEOUT:-240}
MAILPIT_IMAGE=axllent/mailpit:v1.33.1
HOST_NAME=e2e-host
POLL_INTERVAL=5

WORK=$(mktemp -d)
AGENT_PID=""
export CERBRIX_ENV_FILE="$WORK/server.env"

# Native path for programs that are not MSYS aware (docker and the agent on Windows).
native_path() { if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s' "$1"; fi; }

# The configuration repository: a bare repository (what the server fetches from) and a
# normal clone next to it (where the commits are made). The bare repository is mounted at
# /config-repo.git and used as the Git URL. The mount lives in an override file here, not
# in the production compose file.
REPO="$WORK/config-repo.git"
CLONE="$WORK/config-clone"
CONTAINER_REPO=/config-repo.git
cat > "$WORK/compose.e2e.yml" <<YAML
services:
  server:
    depends_on:
      mailpit:
        condition: service_healthy
    volumes:
      - "$(native_path "$REPO"):$CONTAINER_REPO:ro"
  mailpit:
    image: $MAILPIT_IMAGE
    ports:
      - "127.0.0.1:$MAILPIT_PORT:8025"
    healthcheck:
      test: ["CMD", "/mailpit", "readyz"]
      interval: 5s
      timeout: 5s
      retries: 10
YAML

compose() { docker compose -p "$PROJECT" -f "$ROOT/docker-compose.yml" -f "$WORK/compose.e2e.yml" "$@"; }
fail() { echo "e2e: FAIL: $*" >&2; exit 1; }
# Commits without depending on the user's Git configuration; no line ending is rewritten.
g() { git -c user.name=e2e -c user.email=e2e@example.com -c commit.gpgsign=false -c core.autocrlf=false "$@"; }

# --- Mailpit and problems ---------------------------------------------------------------
# Mailpit answers `GET /api/v1/messages` with one JSON document. Each message starts with
# `{"ID":`, so one line per message is enough to look at its recipient and subject with sed
# and grep (no jq: it is not installed everywhere).
mail_lines() {
    curl -fsS "http://127.0.0.1:$MAILPIT_PORT/api/v1/messages?limit=200" 2>/dev/null \
        | tr -d '\r' | sed 's/{"ID":/\n&/g' | grep -F '"Subject":' || true
}
# Prints `recipient<TAB>subject` for every message in Mailpit.
mail_summary() {
    mail_lines | sed -n 's/.*"To":\[{"Name":"[^"]*","Address":"\([^"]*\)"}\].*"Subject":"\([^"]*\)".*/\1\t\2/p'
}
# Prints how many messages to recipient $1 have exactly the subject $2.
mail_count() {
    mail_summary | grep -cxF -- "$(printf '%s\t%s' "$1" "$2")" || true
}
problems() { compose exec -T server cerbrix-server admin problem list "$@" 2>/dev/null | tr -d '\r'; }

# Polls until the command given after the timeout succeeds. On a timeout it says what was
# being waited for; the cleanup then prints the problems and the notification queue.
wait_until() {
    local what=$1 timeout=$2
    shift 2
    local deadline=$((SECONDS + timeout))
    while :; do
        if "$@"; then
            echo "e2e: $what" >&2
            return 0
        fi
        [ -z "$AGENT_PID" ] || kill -0 "$AGENT_PID" 2>/dev/null || fail "the agent exited early while waiting for: $what"
        [ "$SECONDS" -lt "$deadline" ] || fail "timed out after ${timeout}s waiting for: $what"
        sleep 2
    done
}

# Stops the agent (also used by the cleanup); returns 1 if the process is still running.
stop_agent() {
    [ -n "$AGENT_PID" ] || return 0
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
        return 1
    fi
    echo "e2e: agent stopped"
    AGENT_PID=""
}

# The last API request, for the failure output; see the API walk-through at the end.
LAST_REQUEST=""
LAST_STATUS=""
LAST_BODY=""
# Masks session and display tokens in the text on standard input.
mask_tokens() { sed -E 's/cbx_(se|dt|ek|ac)_[A-Za-z0-9_-]+/cbx_\1_<masked>/g'; }

cleanup() {
    status=$?
    set +e
    stop_agent
    if [ "$status" -ne 0 ] && [ -f "$WORK/agent.log" ]; then
        echo "e2e: last agent log lines:" >&2
        tail -n 20 "$WORK/agent.log" >&2
    fi
    # Printed whether or not the stack is kept, so a failed run always shows the state.
    if [ "$status" -ne 0 ] && [ -n "$LAST_REQUEST" ]; then
        echo "e2e: last API request: $LAST_REQUEST -> HTTP $LAST_STATUS" >&2
        echo "e2e: last API response body: $(mask_tokens <<<"$LAST_BODY")" >&2
    fi
    if [ "$status" -ne 0 ]; then
        echo "e2e: admin sync status:" >&2
        compose exec -T server cerbrix-server admin sync status >&2 2>&1
        echo "e2e: admin problem list --all:" >&2
        compose exec -T server cerbrix-server admin problem list --all >&2 2>&1
        echo "e2e: admin notify status:" >&2
        compose exec -T server cerbrix-server admin notify status >&2 2>&1
        echo "e2e: messages in Mailpit (subject, recipient):" >&2
        mail_summary >&2 2>&1
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
command -v git >/dev/null 2>&1 || fail "git is not installed"
command -v cargo >/dev/null 2>&1 || export PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || fail "cargo is not installed"

echo "e2e: building the agent"
cargo build -p cerbrix-agent >&2 || fail "cargo build failed"
AGENT_BIN="$ROOT/target/debug/cerbrix-agent"
[ -x "$AGENT_BIN" ] || AGENT_BIN="$ROOT/target/debug/cerbrix-agent.exe"
[ -x "$AGENT_BIN" ] || fail "agent binary not found in target/debug"

echo "e2e: creating the configuration repository"
git init --bare -q -b main "$REPO" || fail "cannot create the bare repository"
g init -q -b main "$CLONE" || fail "cannot create the working clone"
cp -R examples/config-repo/. "$CLONE/"
# The host of this check uses the template of the example repository. Its own items override
# the template's, with short intervals so that the check stays fast. Its triggers: one that
# is always true (cpu.usage >= 0; the check later changes it to one that is never true), a
# second one that stays open and is acknowledged, and a host-level no-data trigger with the
# same name as the template's "Agent silent", which it replaces (1 minute instead of 3).
write_host_file() {
    cat > "$CLONE/hosts/$HOST_NAME.json" <<JSON
{
  "host": "$HOST_NAME",
  "groups": ["e2e"],
  "templates": ["linux-base"],
  "items": [
    { "key": "cpu.usage", "interval": "5s" },
    { "key": "memory.used_percent", "interval": "5s" },
    { "key": "disk.used_percent", "interval": "5s" }
  ],
  "triggers": [
    { "name": "E2E always", "expression": "cpu.usage $1", "severity": "high" },
    { "name": "E2E second", "expression": "memory.used_percent >= 0", "severity": "warning" },
    { "name": "Agent silent", "expression": "no data for 1m", "severity": "critical" }
  ]
}
JSON
}
write_host_file ">= 0"
# Every host of the installation is mailed about problems of warning and above.
cat > "$CLONE/notifications/e2e.json" <<JSON
{
  "name": "e2e",
  "email": ["ops@example.com"],
  "min_severity": "warning"
}
JSON
# For the API walk-through: a dashboard that shows only this host (the standard problems and
# host-states panels and a graph of its cpu.usage), and a wall display that shows it and
# switches to it on an alarm of warning or above. The display lists the dashboard, so the
# example repository's dashboards cannot take the alarm away from it.
cat > "$CLONE/dashboards/e2e.json" <<JSON
{
  "name": "E2E",
  "hosts": { "names": ["$HOST_NAME"] },
  "panels": [
    { "type": "problems", "x": 0, "y": 0, "w": 8, "h": 6 },
    { "type": "host-states", "x": 8, "y": 0, "w": 4, "h": 3 },
    { "type": "graph", "host": "$HOST_NAME", "item": "cpu.usage", "range": "1h",
      "x": 8, "y": 3, "w": 4, "h": 3 }
  ]
}
JSON
cat > "$CLONE/displays/e2e-wall.json" <<JSON
{
  "name": "E2E wall",
  "show": { "dashboard": "e2e" },
  "switch_on_alarm": { "min_severity": "warning" },
  "dashboards": ["e2e"]
}
JSON
g -C "$CLONE" remote add origin "$(native_path "$REPO")"
g -C "$CLONE" add -A
g -C "$CLONE" commit -q -m "e2e configuration" || fail "cannot commit the configuration"
g -C "$CLONE" push -q origin main 2>&1 || fail "cannot push to the bare repository"

# Throw-away server configuration with a random database password and the Git settings.
DB_PASSWORD=$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')
sed -e "s|^POSTGRES_PASSWORD=.*|POSTGRES_PASSWORD=$DB_PASSWORD|" \
    -e "s|^\(CERBRIX_DATABASE__URL=postgres://cerbrix:\)@|\1$DB_PASSWORD@|" \
    deploy/server.env.example > "$WORK/server.env"
if ! { grep -q "^POSTGRES_PASSWORD=$DB_PASSWORD\$" "$WORK/server.env" \
    && grep -q "cerbrix:$DB_PASSWORD@postgres" "$WORK/server.env"; }; then
    fail "could not fill in the password in the example env file"
fi
cat >> "$WORK/server.env" <<ENV

CERBRIX_GITOPS__ENABLED=true
CERBRIX_GITOPS__URL=$CONTAINER_REPO
CERBRIX_GITOPS__INTERVAL_SECS=5

# E-mail goes to Mailpit (plain SMTP, no credentials) on the compose network.
CERBRIX_SMTP__HOST=mailpit
CERBRIX_SMTP__PORT=1025
CERBRIX_SMTP__ENCRYPTION=none
CERBRIX_SMTP__FROM=cerbrix@example.com
CERBRIX_ALERTING__NODATA_CHECK_SECS=5
ENV

echo "e2e: starting the stack '$PROJECT' (server on port $CERBRIX_SERVER_PORT, VictoriaMetrics on $CERBRIX_VM_PORT)"
compose up -d --build --wait --wait-timeout 300 >&2 || fail "the stack did not become healthy"

# Prints the output of `admin sync status`.
sync_status() { compose exec -T server cerbrix-server admin sync status 2>/dev/null | tr -d '\r'; }

# Waits until the first line of `admin sync status` is $1 and, if given, the output
# contains the text $2. Prints the final status.
wait_for_sync() {
    local want=$1 needle=${2:-} out
    local deadline=$((SECONDS + SYNC_TIMEOUT))
    while :; do
        out=$(sync_status || true)
        if [ "$(printf '%s\n' "$out" | head -n 1)" = "$want" ] \
            && { [ -z "$needle" ] || grep -qF -- "$needle" <<<"$out"; }; then
            printf '%s\n' "$out" >&2
            return 0
        fi
        [ "$SECONDS" -lt "$deadline" ] || fail "sync status did not become \"$want\"${needle:+ with \"$needle\"} within ${SYNC_TIMEOUT}s; last status: $out"
        sleep 2
    done
}

echo "e2e: waiting for the first sync"
FIRST_COMMIT=$(g -C "$CLONE" rev-parse HEAD)
wait_for_sync "in sync" "$FIRST_COMMIT"

echo "e2e: checking that hosts cannot be added by hand"
if ADD_OUTPUT=$(compose exec -T server cerbrix-server admin host add manual-host 2>&1); then
    fail "admin host add succeeded although hosts are managed by Git"
fi
grep -q "managed by Git" <<<"$ADD_OUTPUT" \
    || fail "admin host add failed with an unexpected message: $ADD_OUTPUT"

echo "e2e: creating an enrolment key for $HOST_NAME"
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
        echo "e2e: cerbrix_cpu_usage has $cpu samples, cerbrix_disk_used_percent has $disk samples"
        break
    fi
    [ "$SECONDS" -lt "$deadline" ] || fail "no samples for both series within ${TIMEOUT}s"
    sleep "$POLL_INTERVAL"
done

echo "e2e: pushing a broken host file"
printf '{ "host": "broken", ' > "$CLONE/hosts/broken.json"
g -C "$CLONE" add -A
g -C "$CLONE" commit -q -m "broken host file" || fail "cannot commit the broken file"
g -C "$CLONE" push -q origin main 2>&1 || fail "cannot push the broken file"
BROKEN_COMMIT=$(g -C "$CLONE" rev-parse HEAD)
samples_before=$(sample_count cerbrix_cpu_usage)
wait_for_sync "out of sync" "hosts/broken.json"

# The agent keeps measuring with the last valid configuration.
echo "e2e: waiting for new samples while out of sync (had $samples_before)"
deadline=$((SECONDS + TIMEOUT))
while :; do
    kill -0 "$AGENT_PID" 2>/dev/null || fail "the agent exited early"
    cpu=$(sample_count cerbrix_cpu_usage)
    echo "e2e: cerbrix_cpu_usage=$cpu"
    [ "$cpu" -gt "$samples_before" ] && break
    [ "$SECONDS" -lt "$deadline" ] || fail "no new samples while the configuration was out of sync"
    sleep "$POLL_INTERVAL"
done

echo "e2e: reverting the broken commit"
g -C "$CLONE" revert --no-edit "$BROKEN_COMMIT" >/dev/null || fail "cannot revert the broken commit"
g -C "$CLONE" push -q origin main 2>&1 || fail "cannot push the revert"
wait_for_sync "in sync" "$(g -C "$CLONE" rev-parse HEAD)"


echo "e2e: configuration from Git, out of sync and back in sync, values kept arriving"

# --- Alerting ---------------------------------------------------------------------------
ALWAYS_START="[Cerbrix] HIGH $HOST_NAME: E2E always"
ALWAYS_END="[Cerbrix] Resolved $HOST_NAME: E2E always"
SILENT_START="[Cerbrix] CRITICAL $HOST_NAME: Agent silent"

# True when a line of `admin problem list [--all]` matches the pattern $1. The output is read
# into a variable first, so that grep -q cannot cut a pipe short.
problem_matches() {
    local out
    out=$(problems "${@:2}") && grep -qE -- "$1" <<<"$out"
}
always_open() { problem_matches "  E2E always  open  [0-9a-f]{8}\$"; }
always_mailed() { [ "$(mail_count ops@example.com "$ALWAYS_START")" -ge 1 ]; }
always_closed() {
    local out
    out=$(problems) && ! grep -qF -- "  E2E always  " <<<"$out"
}
always_resolved_listed() { problem_matches "  E2E always  resolved " --all; }
always_end_mailed() { [ "$(mail_count ops@example.com "$ALWAYS_END")" -ge 1 ]; }
second_open() { problem_matches "  E2E second  open  [0-9a-f]{8}\$"; }
second_acked() { problem_matches "  E2E second  acknowledged by admin  "; }
silent_open() { problem_matches "  Agent silent  open  [0-9a-f]{8}\$"; }
silent_mailed() { [ "$(mail_count ops@example.com "$SILENT_START")" -ge 1 ]; }

echo "e2e: checking that a problem opens and is mailed"
wait_until "problem \"E2E always\" is open" "$ALERT_TIMEOUT" always_open
wait_until "a message to ops@example.com with the subject \"$ALWAYS_START\"" "$ALERT_TIMEOUT" always_mailed
wait_until "problem \"E2E second\" is open" "$ALERT_TIMEOUT" second_open

echo "e2e: changing the trigger so that its condition no longer holds"
write_host_file "< 0"
g -C "$CLONE" add -A
g -C "$CLONE" commit -q -m "E2E always can no longer fire" || fail "cannot commit the changed trigger"
g -C "$CLONE" push -q origin main 2>&1 || fail "cannot push the changed trigger"
wait_for_sync "in sync" "$(g -C "$CLONE" rev-parse HEAD)"
wait_until "problem \"E2E always\" is no longer open" "$ALERT_TIMEOUT" always_closed
wait_until "problem \"E2E always\" is listed as resolved" "$ALERT_TIMEOUT" always_resolved_listed
wait_until "a message to ops@example.com with the subject \"$ALWAYS_END\"" "$ALERT_TIMEOUT" always_end_mailed

echo "e2e: acknowledging the second problem"
SECOND_ID=$(problems | sed -n 's/.*  E2E second  open  \([0-9a-f]\{8\}\)$/\1/p' | head -n 1)
[ -n "$SECOND_ID" ] || fail "no id found for the problem \"E2E second\""
ACK_OUTPUT=$(compose exec -T server cerbrix-server admin problem ack "$SECOND_ID" \
    --comment "e2e check" 2>/dev/null | tr -d '\r') || fail "admin problem ack failed"
grep -qE '^problem [0-9a-f-]{36} acknowledged$' <<<"$ACK_OUTPUT" \
    || fail "admin problem ack printed an unexpected answer: $ACK_OUTPUT"
wait_until "problem \"E2E second\" is shown as acknowledged by admin" "$ALERT_TIMEOUT" second_acked

echo "e2e: stopping the agent; the host-level no-data problem must open within ${NODATA_TIMEOUT}s"
stop_agent || fail "the agent could not be stopped"
wait_until "problem \"Agent silent\" is open" "$NODATA_TIMEOUT" silent_open
wait_until "a message to ops@example.com with the subject \"$SILENT_START\"" "$ALERT_TIMEOUT" silent_mailed

# Nothing may be sent twice: give a late duplicate time to show up, then look at all subjects.
# This fixed sleep is not a wait for a condition; it only bounds how long a late duplicate is
# awaited. Every message checked below has already arrived (wait_until above), so the check
# cannot fail because the sleep was too short; a duplicate later than this would go unnoticed.
sleep 10
echo "e2e: messages in Mailpit:"
mail_summary >&2
DUPLICATES=$(mail_summary | sort | uniq -d)
[ -z "$DUPLICATES" ] || fail "a message was sent more than once: $DUPLICATES"
for subject in "$ALWAYS_START" "$ALWAYS_END" "$SILENT_START"; do
    [ "$(mail_count ops@example.com "$subject")" -eq 1 ] \
        || fail "expected exactly one message with the subject \"$subject\""
done

echo "e2e: configuration from Git, values kept arriving, problems opened and resolved with their e-mails, acknowledgement, no-data problem after the agent stopped, no duplicate message"

# --- The API of the web UI --------------------------------------------------------------
# The stack is still running; the agent is stopped, so one problem ("Agent silent") is open
# and unacknowledged and "E2E second" is acknowledged by the admin command. The answers of
# the server are compact JSON, so grep for a fixed text is enough to look into them.
API_URL="http://127.0.0.1:$CERBRIX_SERVER_PORT/api/v1"
ADMIN_JAR="$WORK/admin.cookies"
VIEWER_JAR="$WORK/viewer.cookies"
DISPLAY_ID=e2e-wall
DASHBOARD_ID=e2e
JSON_TYPE='Content-Type: application/json'

# Sends one request and keeps what is needed to judge it in LAST_REQUEST, LAST_STATUS and
# LAST_BODY (the cleanup prints them when the script fails). $1 is a cookie jar file, or
# empty for a request without cookies. The other arguments after the path go to curl.
# Request bodies and secrets are passed in files under $WORK, never on the command line.
api() {
    local jar=$1 method=$2 path=$3
    shift 3
    local cookies=()
    [ -z "$jar" ] || cookies=(-b "$jar" -c "$jar")
    LAST_REQUEST="$method /api/v1$path"
    LAST_STATUS=$(curl -sS -o "$WORK/api.body" -w '%{http_code}' -X "$method" \
        ${cookies[@]+"${cookies[@]}"} "$@" "$API_URL$path") \
        || fail "curl could not complete: $LAST_REQUEST"
    LAST_BODY=$(tr -d '\r' <"$WORK/api.body")
}
# Like api, with the JSON text $4 as the request body.
api_json() {
    local jar=$1 method=$2 path=$3 body=$4
    (umask 077 && printf '%s' "$body" >"$WORK/api.request")
    api "$jar" "$method" "$path" -H "$JSON_TYPE" --data-binary "@$WORK/api.request"
    rm -f "$WORK/api.request"
}
expect_status() {
    [ "$LAST_STATUS" = "$1" ] || fail "$LAST_REQUEST answered HTTP $LAST_STATUS, expected $1"
}
expect_body() {
    grep -qF -- "$1" <<<"$LAST_BODY" || fail "the answer to $LAST_REQUEST does not contain: $1"
}
# Prints the sum of the numbers that follow "$1": in the text on standard input.
sum_of() { grep -oE "\"$1\":[0-9]+" | awk -F: '{ total += $2 } END { print total + 0 }'; }
# Logs in as user $2 with password $3 and the cookie jar $1.
login() {
    local jar=$1
    (umask 077 && printf '{"name":"%s","password":"%s"}' "$2" "$3" >"$WORK/login.json")
    api "$jar" POST /session -H "$JSON_TYPE" --data-binary "@$WORK/login.json"
    rm -f "$WORK/login.json"
}

echo "e2e: API: creating an admin and logging in"
ADMIN_OUTPUT=$(compose exec -T server cerbrix-server admin user create e2e-admin --role admin \
    2>/dev/null) || fail "admin user create failed"
ADMIN_PASSWORD=$(printf '%s\n' "$ADMIN_OUTPUT" | tr -d '\r' | sed -n 's/^password: //p')
unset ADMIN_OUTPUT
[ -n "$ADMIN_PASSWORD" ] || fail "no password found in the output of admin user create"
login "$ADMIN_JAR" e2e-admin "$ADMIN_PASSWORD"
unset ADMIN_PASSWORD
expect_status 200
expect_body '"name":"e2e-admin","role":"admin"'
api "$ADMIN_JAR" GET /session
expect_status 200
expect_body '"name":"e2e-admin","role":"admin"'

echo "e2e: API: the status matches the problems of the admin command"
api "$ADMIN_JAR" GET /status
expect_status 200
expect_body '"status":"in_sync"'
expect_body '"gitops":true'
STATUS_PROBLEMS=${LAST_BODY#*\"problems\":}
CLI_PROBLEMS=$(problems)
CLI_OPEN=$(grep -vcx 'no open problems' <<<"$CLI_PROBLEMS" || true)
CLI_UNACKED=$(grep -cE '  open  [0-9a-f]{8}$' <<<"$CLI_PROBLEMS" || true)
API_OPEN=$(sum_of open <<<"$STATUS_PROBLEMS")
API_UNACKED=$(sum_of unacknowledged <<<"$STATUS_PROBLEMS")
[ "$CLI_OPEN" -ge 1 ] || fail "the admin command lists no open problem"
[ "$API_OPEN" = "$CLI_OPEN" ] \
    || fail "the status counts $API_OPEN open problems, the admin command lists $CLI_OPEN"
[ "$API_UNACKED" = "$CLI_UNACKED" ] \
    || fail "the status counts $API_UNACKED unacknowledged problems, the admin command lists $CLI_UNACKED"

echo "e2e: API: the host with its state, and a series with points"
api "$ADMIN_JAR" GET "/hosts?search=$HOST_NAME"
expect_status 200
expect_body "\"name\":\"$HOST_NAME\""
expect_body '"state":{"severity":'
api "$ADMIN_JAR" GET "/hosts/$HOST_NAME/series?item=cpu.usage&range=1h"
expect_status 200
expect_body '"points":[['

echo "e2e: API: acknowledging a problem"
api "$ADMIN_JAR" GET "/problems?host=$HOST_NAME"
expect_status 200
expect_body '"trigger":"Agent silent"'
expect_body '"trigger":"E2E second"'
SECOND_UUID=$(sed -n 's/.*{"id":"\([0-9a-f-]\{36\}\)","host":"[^"]*","trigger":"E2E second".*/\1/p' <<<"$LAST_BODY")
[ -n "$SECOND_UUID" ] || fail "no id found for the problem \"E2E second\" in the answer to $LAST_REQUEST"
api_json "$ADMIN_JAR" POST "/problems/$SECOND_UUID/ack" '{"comment":"e2e api check"}'
expect_status 200
expect_body '"acknowledged_by":"e2e-admin"'
expect_body '"ack_comment":"e2e api check"'
api "$ADMIN_JAR" GET "/problems/$SECOND_UUID"
expect_status 200
expect_body '"acknowledged_by":"e2e-admin"'

echo "e2e: API: the dashboard shows the open problem and its graph has points"
api "$ADMIN_JAR" GET /dashboards
expect_status 200
expect_body "\"id\":\"$DASHBOARD_ID\""
api "$ADMIN_JAR" GET "/dashboards/$DASHBOARD_ID"
expect_status 200
expect_body '"type":"problems"'
expect_body '"trigger":"Agent silent"'
expect_body '"type":"graph"'
expect_body '"points":[['
expect_body '"metrics_available":true'

echo "e2e: API: a display token opens its own display and nothing else"
api "$ADMIN_JAR" POST "/displays/$DISPLAY_ID/token"
expect_status 201
DISPLAY_TOKEN=$(sed -n 's/.*"token":"\([^"]*\)".*/\1/p' <<<"$LAST_BODY")
[ -n "$DISPLAY_TOKEN" ] || fail "no token in the answer to $LAST_REQUEST"
(umask 077 && printf 'Authorization: Bearer %s\n' "$DISPLAY_TOKEN" >"$WORK/display.header")
unset DISPLAY_TOKEN
api "" GET "/display/$DISPLAY_ID" -H "@$WORK/display.header"
expect_status 200
expect_body "\"id\":\"$DISPLAY_ID\""
expect_body "\"target\":\"$DASHBOARD_ID\""
expect_body '"status":"in_sync"'
api "" GET "/display/$DISPLAY_ID/dashboards/$DASHBOARD_ID" -H "@$WORK/display.header"
expect_status 200
expect_body '"name":"E2E"'
api "" GET /hosts -H "@$WORK/display.header"
expect_status 401
api "" GET /display/another-display -H "@$WORK/display.header"
expect_status 401

echo "e2e: API: a request without a session and a viewer"
api "" GET /hosts
expect_status 401
expect_body '"error":"unauthorized"'
VIEWER_PASSWORD=$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \n')
api_json "$ADMIN_JAR" POST /users \
    "{\"name\":\"e2e-viewer\",\"role\":\"viewer\",\"password\":\"$VIEWER_PASSWORD\"}"
expect_status 201
VIEWER_UUID=$(sed -n 's/.*"id":"\([0-9a-f-]\{36\}\)".*/\1/p' <<<"$LAST_BODY")
[ -n "$VIEWER_UUID" ] || fail "no id in the answer to $LAST_REQUEST"
login "$VIEWER_JAR" e2e-viewer "$VIEWER_PASSWORD"
unset VIEWER_PASSWORD
expect_status 200
expect_body '"role":"viewer"'
api "$VIEWER_JAR" GET /hosts
expect_status 200
api_json "$VIEWER_JAR" POST /users '{"name":"e2e-other","role":"viewer"}'
expect_status 403
expect_body '"error":"forbidden"'

echo "e2e: API: cleaning up the viewer, the token and the session"
api "$ADMIN_JAR" DELETE "/users/$VIEWER_UUID"
expect_status 204
api "$VIEWER_JAR" GET /session
expect_status 401
api "$ADMIN_JAR" DELETE "/displays/$DISPLAY_ID/token"
expect_status 204
api "" GET "/display/$DISPLAY_ID" -H "@$WORK/display.header"
expect_status 401
cp "$ADMIN_JAR" "$WORK/admin.cookies.old"
api "$ADMIN_JAR" DELETE /session
expect_status 204
api "$WORK/admin.cookies.old" GET /session
expect_status 401
rm -f "$ADMIN_JAR" "$VIEWER_JAR" "$WORK/admin.cookies.old" "$WORK/display.header" "$WORK/api.body"
LAST_REQUEST=""

echo "e2e: OK: configuration from Git, values kept arriving, problems opened and resolved with their e-mails, acknowledgement, no-data problem after the agent stopped, no duplicate message, API walk-through (login, status, hosts, series, problems, dashboard, display token, access rules)"
