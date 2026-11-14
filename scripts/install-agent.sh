#!/bin/sh
# Installs or upgrades the Cerbrix agent on a Linux host with systemd. Run as root.
#
#   install-agent.sh --server-url URL --host NAME [--ca-file PEM] [--version X.Y.Z]
#                    [--enrollment-key KEY] [--dry-run]
#
# The enrolment key is best passed in the environment variable CERBRIX_ENROLLMENT_KEY,
# so that it does not end up in the shell history or in the process list.
#
# Other environment variables:
#   CERBRIX_RELEASES_URL        base URL of the releases (default: the GitHub releases of
#                               the Cerbrix repository); for testing
#   CERBRIX_ALLOW_HTTP_DOWNLOAD set to 1 to accept an http:// CERBRIX_RELEASES_URL; for
#                               testing only, the download is then not protected
#
# Running it again upgrades the binary and the unit file and keeps an existing
# /etc/cerbrix/agent.toml unchanged.
set -eu
# Never trace: with `sh -x` the enrolment key would show up in the trace.
{ set +x; } 2>/dev/null

DEFAULT_RELEASES_URL="https://github.com/TrisMarzec/Cerbrix/releases"
BIN_PATH=/usr/local/bin/cerbrix-agent
CONFIG_DIR=/etc/cerbrix
CONFIG_FILE=$CONFIG_DIR/agent.toml
CA_DEST=$CONFIG_DIR/ca.pem
UNIT_PATH=/etc/systemd/system/cerbrix-agent.service
SERVICE_USER=cerbrix
SERVICE=cerbrix-agent

SERVER_URL=""
HOST_NAME=""
KEY=${CERBRIX_ENROLLMENT_KEY:-}
CA_FILE=""
VERSION=""
DRY_RUN=0
TMP=""
FETCH=""

say() { printf 'install-agent: %s\n' "$*"; }
warn() { printf 'install-agent: warning: %s\n' "$*" >&2; }
die() {
    printf 'install-agent: error: %s\n' "$*" >&2
    exit 1
}
would() { printf 'would: %s\n' "$*"; }

cleanup() {
    if [ -n "$TMP" ]; then
        rm -rf "$TMP"
    fi
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

usage() {
    cat <<'EOF'
Usage: install-agent.sh [options]

  --server-url URL        https URL of the Cerbrix server
  --host NAME             name of this host as registered on the server
  --enrollment-key KEY    enrolment key (prefer the environment variable
                          CERBRIX_ENROLLMENT_KEY, it stays out of the shell history)
  --ca-file PATH          PEM certificate of the CA that signed the server certificate
  --version X.Y.Z         agent version to install (default: the latest release)
  --dry-run               print what would be done, change nothing (no root needed)
  -h, --help              show this help

--server-url, --host and a key are required for the first installation. If
/etc/cerbrix/agent.toml exists it is kept unchanged and they are optional.
EOF
}

need_value() {
    [ "$1" -ge 2 ] || die "option $2 needs a value (see --help)"
}

while [ $# -gt 0 ]; do
    case $1 in
        --server-url) need_value $# "$1"; SERVER_URL=$2; shift 2 ;;
        --host) need_value $# "$1"; HOST_NAME=$2; shift 2 ;;
        --enrollment-key) need_value $# "$1"; KEY=$2; shift 2 ;;
        --ca-file) need_value $# "$1"; CA_FILE=$2; shift 2 ;;
        --version) need_value $# "$1"; VERSION=$2; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
done

# ---- validation (nothing is changed before all of this passes) --------------------

[ "$(uname -s)" = Linux ] || die "this script supports Linux only"

case $(uname -m) in
    x86_64 | amd64) TARGET=x86_64-unknown-linux-musl ;;
    aarch64 | arm64) TARGET=aarch64-unknown-linux-musl ;;
    *) die "unsupported architecture: $(uname -m) (supported: x86_64, aarch64)" ;;
esac

if [ "$DRY_RUN" -eq 0 ]; then
    [ "$(id -u)" -eq 0 ] || die "this script must run as root (try sudo)"
fi

if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    :
elif [ "$DRY_RUN" -eq 1 ]; then
    warn "systemd not found; a real run would refuse to continue"
else
    die "systemd not found; the agent is installed as a systemd service"
fi

RELEASES_URL=${CERBRIX_RELEASES_URL:-$DEFAULT_RELEASES_URL}
RELEASES_URL=${RELEASES_URL%/}
PROTO='=https'
case $RELEASES_URL in
    https://*) ;;
    http://*)
        [ "${CERBRIX_ALLOW_HTTP_DOWNLOAD:-0}" = 1 ] \
            || die "CERBRIX_RELEASES_URL must be an https URL"
        PROTO='=http,https'
        warn "plain http downloads are enabled; use this for testing only"
        ;;
    *) die "CERBRIX_RELEASES_URL must be an https URL" ;;
esac

if command -v curl >/dev/null 2>&1; then
    FETCH=curl
elif command -v wget >/dev/null 2>&1; then
    FETCH=wget
else
    [ "$DRY_RUN" -eq 1 ] || die "neither curl nor wget found; install one of them"
    warn "neither curl nor wget found; a real run would refuse to continue"
fi
if [ "$DRY_RUN" -eq 0 ]; then
    command -v sha256sum >/dev/null 2>&1 || die "sha256sum not found"
    command -v tar >/dev/null 2>&1 || die "tar not found"
fi

valid_version() {
    case $1 in
        '' | *[!0-9A-Za-z.+-]*) return 1 ;;
        [0-9]*.[0-9]*.[0-9]*) return 0 ;;
        *) return 1 ;;
    esac
}

if [ -n "$VERSION" ]; then
    VERSION=${VERSION#v}
    valid_version "$VERSION" || die "invalid version (expected X.Y.Z)"
fi

KEEP_CONFIG=0
if [ -e "$CONFIG_FILE" ]; then
    KEEP_CONFIG=1
fi

if [ "$KEEP_CONFIG" -eq 1 ]; then
    if [ -n "$SERVER_URL" ] || [ -n "$HOST_NAME" ] || [ -n "$KEY" ] || [ -n "$CA_FILE" ]; then
        say "$CONFIG_FILE exists and is kept unchanged; --server-url, --host, --ca-file and the key are ignored"
    fi
    SERVER_URL=""
    HOST_NAME=""
    KEY=""
    CA_FILE=""
else
    [ -n "$SERVER_URL" ] || die "--server-url is required (no $CONFIG_FILE yet)"
    [ -n "$HOST_NAME" ] || die "--host is required (no $CONFIG_FILE yet)"
    if [ -z "$KEY" ] && [ "$DRY_RUN" -eq 0 ]; then
        die "an enrolment key is required: set CERBRIX_ENROLLMENT_KEY (preferred) or use --enrollment-key"
    fi
    # The values are written into a TOML file; allow only characters that need no escaping.
    case $SERVER_URL in
        https://*) ;;
        *) die "--server-url must be an https URL" ;;
    esac
    case $SERVER_URL in
        *[!A-Za-z0-9._~:/?#@%+=\&-]*) die "--server-url contains unsupported characters" ;;
    esac
    case $HOST_NAME in
        [!A-Za-z0-9]* | *[!A-Za-z0-9._-]*)
            die "--host must use letters, digits, '.', '_' or '-' and start with a letter or digit"
            ;;
    esac
    case $KEY in
        *[!A-Za-z0-9._~+/=-]*) die "the enrolment key contains unsupported characters" ;;
    esac
fi

if [ -n "$CA_FILE" ]; then
    [ -f "$CA_FILE" ] && [ -r "$CA_FILE" ] || die "cannot read the CA file: $CA_FILE"
    grep -q 'BEGIN CERTIFICATE' "$CA_FILE" || die "the CA file is not a PEM certificate: $CA_FILE"
fi

# ---- helpers ----------------------------------------------------------------------

# fetch URL DEST
fetch() {
    if [ "$FETCH" = curl ]; then
        curl --fail --location --proto "$PROTO" --proto-redir "$PROTO" --tlsv1.2 \
            --silent --show-error --retry 3 --output "$2" "$1"
    elif [ "$PROTO" = '=https' ]; then
        wget --https-only --secure-protocol=TLSv1_2 --quiet --tries=3 -O "$2" "$1"
    else
        wget --quiet --tries=3 -O "$2" "$1"
    fi
}

# Prints the version of the latest release, taken from the redirect of /releases/latest.
latest_version() {
    if [ "$FETCH" = curl ]; then
        url=$(curl --fail --location --proto "$PROTO" --proto-redir "$PROTO" --tlsv1.2 \
            --silent --show-error --output /dev/null --write-out '%{url_effective}' \
            "$RELEASES_URL/latest") || return 1
    else
        if [ "$PROTO" = '=https' ]; then
            url=$(wget --https-only --secure-protocol=TLSv1_2 --max-redirect=0 \
                --server-response -O /dev/null "$RELEASES_URL/latest" 2>&1 || true)
        else
            url=$(wget --max-redirect=0 --server-response -O /dev/null \
                "$RELEASES_URL/latest" 2>&1 || true)
        fi
        url=$(printf '%s\n' "$url" | sed -n 's/^ *[Ll]ocation: *//p' | head -n 1 | tr -d '\r')
    fi
    tag=${url##*/}
    tag=${tag#v}
    valid_version "$tag" || return 1
    printf '%s\n' "$tag"
}

# ---- plan -------------------------------------------------------------------------

if [ -z "$VERSION" ]; then
    if [ "$DRY_RUN" -eq 1 ]; then
        would "look up the latest version at $RELEASES_URL/latest"
        VERSION="<latest>"
    else
        say "looking up the latest version"
        VERSION=$(latest_version) || die "cannot determine the latest version from $RELEASES_URL/latest (use --version)"
    fi
fi

ARCHIVE=cerbrix-agent-$VERSION-$TARGET.tar.gz
BASE=$RELEASES_URL/download/v$VERSION

if [ "$DRY_RUN" -eq 1 ]; then
    would "download $BASE/$ARCHIVE"
    would "download $BASE/$ARCHIVE.sha256"
    would "verify the SHA-256 checksum and abort on mismatch"
    would "extract cerbrix-agent and cerbrix-agent.service from the archive"
    would "install the binary to $BIN_PATH (mode 0755, root:root)"
    if getent passwd "$SERVICE_USER" >/dev/null 2>&1; then
        would "keep the existing system user and group $SERVICE_USER"
    else
        would "create the system user and group $SERVICE_USER (no login, no home)"
    fi
    would "create the directory $CONFIG_DIR (mode 0755, root:root)"
    if [ "$KEEP_CONFIG" -eq 1 ]; then
        would "keep the existing $CONFIG_FILE unchanged"
    else
        if [ -n "$CA_FILE" ]; then
            would "copy $CA_FILE to $CA_DEST (mode 0644, root:root)"
        fi
        if [ -n "$KEY" ]; then
            would "write $CONFIG_FILE (mode 0640, root:$SERVICE_USER) with server_url=$SERVER_URL host=$HOST_NAME, an enrolment key (not shown)"
        else
            would "write $CONFIG_FILE (mode 0640, root:$SERVICE_USER) with server_url=$SERVER_URL host=$HOST_NAME; no enrolment key was given yet (a real run needs CERBRIX_ENROLLMENT_KEY or --enrollment-key)"
        fi
    fi
    would "install the unit file to $UNIT_PATH (mode 0644)"
    would "run: systemctl daemon-reload"
    would "run: systemctl enable $SERVICE"
    would "run: systemctl restart $SERVICE"
    exit 0
fi

# ---- download and verify (nothing on the system is changed until this succeeded) --

TMP=$(mktemp -d)
chmod 0700 "$TMP"

say "downloading $ARCHIVE"
fetch "$BASE/$ARCHIVE" "$TMP/$ARCHIVE" || die "download failed: $BASE/$ARCHIVE"
fetch "$BASE/$ARCHIVE.sha256" "$TMP/$ARCHIVE.sha256" || die "download failed: $BASE/$ARCHIVE.sha256"

# Build the check line ourselves so that only the hash is taken from the downloaded file.
EXPECTED=$(sed -n '1s/^\([0-9A-Fa-f]\{64\}\)\([ ].*\)\{0,1\}$/\1/p' "$TMP/$ARCHIVE.sha256")
[ -n "$EXPECTED" ] || die "the checksum file $ARCHIVE.sha256 is malformed"
printf '%s  %s\n' "$EXPECTED" "$ARCHIVE" | (cd "$TMP" && sha256sum -c - >/dev/null 2>&1) \
    || die "SHA-256 checksum mismatch for $ARCHIVE; nothing was installed"
say "checksum verified"

mkdir "$TMP/extract"
tar -xzf "$TMP/$ARCHIVE" -C "$TMP/extract" cerbrix-agent cerbrix-agent.service \
    || die "the archive does not contain cerbrix-agent and cerbrix-agent.service"
for f in cerbrix-agent cerbrix-agent.service; do
    if [ -L "$TMP/extract/$f" ] || [ ! -f "$TMP/extract/$f" ]; then
        die "unexpected entry type for $f in the archive"
    fi
done

# ---- install ----------------------------------------------------------------------

if ! getent group "$SERVICE_USER" >/dev/null 2>&1; then
    command -v groupadd >/dev/null 2>&1 || die "groupadd not found; cannot create the group $SERVICE_USER"
    groupadd --system "$SERVICE_USER"
fi
if getent passwd "$SERVICE_USER" >/dev/null 2>&1; then
    say "system user $SERVICE_USER already exists"
else
    command -v useradd >/dev/null 2>&1 || die "useradd not found; cannot create the user $SERVICE_USER"
    NOLOGIN=/bin/false
    for candidate in /usr/sbin/nologin /sbin/nologin; do
        if [ -x "$candidate" ]; then
            NOLOGIN=$candidate
            break
        fi
    done
    useradd --system --gid "$SERVICE_USER" --no-create-home --home-dir /nonexistent \
        --shell "$NOLOGIN" "$SERVICE_USER"
    say "created system user $SERVICE_USER"
fi

install -m 0755 -o root -g root "$TMP/extract/cerbrix-agent" "$BIN_PATH"
say "installed $BIN_PATH"

install -d -m 0755 -o root -g root "$CONFIG_DIR"
if [ "$KEEP_CONFIG" -eq 1 ]; then
    say "kept the existing $CONFIG_FILE unchanged"
else
    if [ -n "$CA_FILE" ]; then
        install -m 0644 -o root -g root "$CA_FILE" "$CA_DEST"
    fi
    # Created under a restrictive umask, so the key is never readable by others, not
    # even for a moment. printf is a shell builtin: the key is on no command line.
    NEW_CONFIG=$CONFIG_DIR/.agent.toml.new
    (
        umask 077
        {
            printf 'server_url = "%s"\n' "$SERVER_URL"
            printf 'host = "%s"\n' "$HOST_NAME"
            printf 'enrollment_key = "%s"\n' "$KEY"
            if [ -n "$CA_FILE" ]; then
                printf 'ca_file = "%s"\n' "$CA_DEST"
            fi
        } >"$NEW_CONFIG"
    )
    chown root:"$SERVICE_USER" "$NEW_CONFIG"
    chmod 0640 "$NEW_CONFIG"
    mv "$NEW_CONFIG" "$CONFIG_FILE"
    say "wrote $CONFIG_FILE"
fi

install -m 0644 -o root -g root "$TMP/extract/cerbrix-agent.service" "$UNIT_PATH"
say "installed $UNIT_PATH"

systemctl daemon-reload
systemctl enable "$SERVICE" >/dev/null 2>&1 || die "systemctl enable $SERVICE failed"
systemctl restart "$SERVICE" || die "systemctl restart $SERVICE failed (see: journalctl -u $SERVICE)"
sleep 2
if systemctl is-active --quiet "$SERVICE"; then
    say "the $SERVICE service is running"
else
    warn "the $SERVICE service is not running; see: journalctl -u $SERVICE"
fi
say "done; follow the log with: journalctl -u $SERVICE -f"
