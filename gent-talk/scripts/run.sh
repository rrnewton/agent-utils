#!/usr/bin/env bash
# Build and run the gent-talk container, with the checks that make a bad start fail HERE instead
# of five minutes later inside a container that came up and quietly does not work.
#
# What it does, in order:
#   1. Refuses to run if a gent-talk container is already up (see --shutdown / --restart).
#   2. Loads configuration and verifies every REQUIRED variable is actually set, by name.
#   3. Optionally (off by default) makes sure the cloudflared tunnel unit is running.
#   4. Rebuilds the image.
#   5. Runs the container.
#
# Usage:
#   scripts/run.sh [--shutdown] [--restart] [--config FILE] [--tunnel|--no-tunnel]
#                  [--tag TAG] [--port PORT] [--dry-run] [--status] [--help]
#
# Flags:
#   --shutdown     Stop the running gent-talk container and EXIT (it does not go on to rebuild or
#                  relaunch). Use --restart for stop-then-relaunch.
#   --restart      Stop the running gent-talk container, then continue with rebuild and relaunch.
#   --config FILE  Use exactly this configuration file and no other. Missing file is an error.
#   --tunnel       Force the cloudflared tunnel check ON for this run.
#   --no-tunnel    Force the cloudflared tunnel check OFF for this run.
#   --tag TAG      Image tag to build and run (default: v0, or $GENT_TALK_IMAGE_TAG).
#   --port PORT    Host port to publish on (default: 8080, or $GENT_TALK_HOST_PORT).
#   --dry-run      Run every check and print the build/run commands, but build and start nothing.
#   --status       Report what is running, plus config and tunnel state, then exit.
#   --help         This text.
#
# Configuration precedence, HIGHEST first:
#   1. Variables already exported in your shell environment. These always win.
#   2. --config FILE, if given. When passed, no other file is read.
#   3. gent-talk/.gent-talk.env        — per-checkout overlay (gitignored).
#   4. ~/.config/gent-talk/env         — per-user base.
# Files 3 and 4 are LAYERED: the base is read first and the per-checkout file overrides individual
# variables in it. An empty or partial per-checkout file therefore cannot break a working per-user
# setup. Copy gent-talk/gent-talk.env.example to get started; it documents every variable.
#
# This script never prints the value of a credential. It reports variables by NAME only.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
GENT_TALK_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"

USER_CONFIG_DEFAULT="${XDG_CONFIG_HOME:-$HOME/.config}/gent-talk/env"
LOCAL_CONFIG_DEFAULT="$GENT_TALK_DIR/.gent-talk.env"
EXAMPLE_CONFIG="$GENT_TALK_DIR/gent-talk.env.example"

# Required for the server to work at all. A container that starts without one of these is the
# failure mode this script exists to prevent.
REQUIRED_VARS=(
    GENT_TALK_DISCORD_BOT_TOKEN
    GENT_TALK_READ_TOKEN
    GENT_TALK_WRITE_TOKEN
    GENT_TALK_CHANNELS
)
# Passed through to the container when set, ignored when not.
OPTIONAL_VARS=(
    GENT_TALK_ELEVENLABS_API_KEY
    GENT_TALK_ELEVENLABS_AGENT_ID
    GENT_TALK_ELEVENLABS_API_BASE
    GENT_TALK_PUBLIC_BASE_URL
    GENT_TALK_SKIP_STARTUP_PROBE
    RUST_LOG
)
# Settings for this launcher itself (not for the server). They may be set in the same config file.
LAUNCHER_VARS=(
    GENT_TALK_IMAGE_NAME
    GENT_TALK_IMAGE_TAG
    GENT_TALK_HOST_PORT
    GENT_TALK_HOST_ADDR
    GENT_TALK_TUNNEL_ENABLED
    GENT_TALK_TUNNEL_UNIT
    GENT_TALK_ENGINE
)

CONFIG_FILE=""
SHUTDOWN=0
RESTART=0
DRY_RUN=0
STATUS_ONLY=0
TUNNEL_OVERRIDE=""
TAG_OVERRIDE=""
PORT_OVERRIDE=""

# The header comment above IS the help text: printed from line 2 up to the first non-comment
# line, so it cannot drift out of sync with the flags it documents.
usage() { awk 'NR>1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"; }

die() { echo "" >&2; echo "gent-talk run.sh: $*" >&2; exit 1; }
note() { echo "  $*"; }
step() { echo "==> $*"; }

while [ $# -gt 0 ]; do
    case "$1" in
        --shutdown) SHUTDOWN=1; shift ;;
        --restart) RESTART=1; shift ;;
        --config) CONFIG_FILE="${2:?--config needs a file path}"; shift 2 ;;
        --tunnel) TUNNEL_OVERRIDE=1; shift ;;
        --no-tunnel) TUNNEL_OVERRIDE=0; shift ;;
        --tag) TAG_OVERRIDE="${2:?--tag needs a value}"; shift 2 ;;
        --port) PORT_OVERRIDE="${2:?--port needs a value}"; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        --status) STATUS_ONLY=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unrecognized argument: $1" >&2; echo "" >&2; usage >&2; exit 2 ;;
    esac
done

if [ "$SHUTDOWN" = 1 ] && [ "$RESTART" = 1 ]; then
    die "--shutdown and --restart contradict each other. --shutdown stops and exits; --restart stops and then relaunches."
fi

# ---------------------------------------------------------------------------------------------
# 2 (first half). Load configuration.
#
# Precedence is implemented by remembering which variables the CALLING environment already set,
# sourcing the files lowest-precedence first, then restoring the caller's values on top. That is
# what makes an empty per-checkout file harmless.
# ---------------------------------------------------------------------------------------------

ALL_VARS=("${REQUIRED_VARS[@]}" "${OPTIONAL_VARS[@]}" "${LAUNCHER_VARS[@]}")

declare -A PRESET_VALUES=()
for var in "${ALL_VARS[@]}"; do
    if [ -n "${!var+x}" ]; then PRESET_VALUES["$var"]="${!var}"; fi
done

SOURCED_FILES=()
source_config() {
    local file="$1"
    # `set -u` is relaxed only across the source: config files legitimately reference variables
    # that may not be set yet (e.g. VAR="${VAR:-default}" style lines).
    set +u
    # shellcheck disable=SC1090
    . "$file"
    set -u
    SOURCED_FILES+=("$file")
}

if [ -n "$CONFIG_FILE" ]; then
    [ -f "$CONFIG_FILE" ] || die "--config file not found: $CONFIG_FILE"
    [ -r "$CONFIG_FILE" ] || die "--config file is not readable: $CONFIG_FILE"
    source_config "$CONFIG_FILE"
else
    if [ -f "$USER_CONFIG_DEFAULT" ]; then source_config "$USER_CONFIG_DEFAULT"; fi
    if [ -f "$LOCAL_CONFIG_DEFAULT" ]; then source_config "$LOCAL_CONFIG_DEFAULT"; fi
fi

for var in "${!PRESET_VALUES[@]}"; do
    printf -v "$var" '%s' "${PRESET_VALUES[$var]}"
done

if [ "${#SOURCED_FILES[@]}" -eq 0 ]; then
    die "no configuration file found.

Looked for, in increasing precedence:
  $USER_CONFIG_DEFAULT
  $LOCAL_CONFIG_DEFAULT

Create one with:
  cp $EXAMPLE_CONFIG $LOCAL_CONFIG_DEFAULT
  \$EDITOR $LOCAL_CONFIG_DEFAULT

That example documents every variable and marks which are required. The per-checkout file is
gitignored; never commit a filled-in copy."
fi

# ---------------------------------------------------------------------------------------------
# Launcher settings, after config load so the config file can set them.
# ---------------------------------------------------------------------------------------------

IMAGE_NAME="${GENT_TALK_IMAGE_NAME:-gent-talk}"
IMAGE_TAG="${TAG_OVERRIDE:-${GENT_TALK_IMAGE_TAG:-v0}}"
HOST_PORT="${PORT_OVERRIDE:-${GENT_TALK_HOST_PORT:-8080}}"
HOST_ADDR="${GENT_TALK_HOST_ADDR:-127.0.0.1}"
TUNNEL_UNIT="${GENT_TALK_TUNNEL_UNIT:-cloudflared-gent-talk.service}"
ENGINE="${GENT_TALK_ENGINE:-podman}"
IMAGE_REF="${IMAGE_NAME}:${IMAGE_TAG}"

case "$HOST_PORT" in
    ''|*[!0-9]*) die "host port must be a number, got: $HOST_PORT" ;;
esac

TUNNEL_ENABLED_RAW="${TUNNEL_OVERRIDE:-${GENT_TALK_TUNNEL_ENABLED:-0}}"
case "$(printf '%s' "$TUNNEL_ENABLED_RAW" | tr '[:upper:]' '[:lower:]')" in
    1|true|yes|on) TUNNEL_ENABLED=1 ;;
    0|false|no|off|'') TUNNEL_ENABLED=0 ;;
    *) die "GENT_TALK_TUNNEL_ENABLED must be one of 1/0/true/false/yes/no/on/off, got: $TUNNEL_ENABLED_RAW" ;;
esac

command -v "$ENGINE" >/dev/null 2>&1 \
    || die "container engine '$ENGINE' not found on PATH. Install podman, or set GENT_TALK_ENGINE=docker."

# ---------------------------------------------------------------------------------------------
# 1. Already-running check.
#
# Matched by IMAGE reference or by PUBLISHED HOST PORT — never by container name. `podman run`
# without --name invents a new name every launch, so a name match is guaranteed to go stale, and
# matching a name prefix would also sweep up unrelated containers.
# ---------------------------------------------------------------------------------------------

running_containers() {
    "$ENGINE" ps --format '{{.ID}}|{{.Image}}|{{.Names}}|{{.Ports}}|{{.Status}}' 2>/dev/null \
        | awk -F'|' -v img="$IMAGE_REF" -v port="$HOST_PORT" '
            {
                image_match = ($2 == img) || (index($2, "/" img) == length($2) - length(img))
                # A published port renders as e.g. "127.0.0.1:8080->8080/tcp"; requiring the
                # colon immediately before the number keeps ":8080->" from matching ":18080->".
                port_match = (index($4, ":" port "->") > 0)
                if (image_match || port_match) print $0
            }'
}

stop_running() {
    local found="$1" id name
    while IFS='|' read -r id _image name _ports _status; do
        [ -n "$id" ] || continue
        step "Stopping container $name ($id)"
        if [ "$DRY_RUN" = 1 ]; then
            note "(dry run) $ENGINE stop $id"
        else
            "$ENGINE" stop "$id" >/dev/null \
                || die "failed to stop container $id ($name). Stop it by hand: $ENGINE stop $id"
        fi
    done <<< "$found"
}

FOUND="$(running_containers || true)"

if [ "$STATUS_ONLY" = 1 ]; then
    step "gent-talk status"
    note "image:        $IMAGE_REF"
    note "publish:      ${HOST_ADDR}:${HOST_PORT}"
    note "config files: ${SOURCED_FILES[*]}"
    note "tunnel check: $([ "$TUNNEL_ENABLED" = 1 ] && echo "enabled (unit $TUNNEL_UNIT)" || echo "disabled")"
    if [ -n "$FOUND" ]; then
        note "running:"
        while IFS='|' read -r id image name ports status; do
            [ -n "$id" ] || continue
            note "  $id  $image  name=$name  ports=$ports  $status"
        done <<< "$FOUND"
    else
        note "running:      nothing matching $IMAGE_REF or host port $HOST_PORT"
    fi
    exit 0
fi

if [ -n "$FOUND" ]; then
    if [ "$SHUTDOWN" = 1 ] || [ "$RESTART" = 1 ]; then
        stop_running "$FOUND"
        if [ "$SHUTDOWN" = 1 ]; then
            step "Stopped. Not rebuilding or relaunching (that is what --shutdown means; use --restart to stop and relaunch)."
            exit 0
        fi
    else
        echo "" >&2
        echo "gent-talk is already running:" >&2
        while IFS='|' read -r id image name ports status; do
            [ -n "$id" ] || continue
            echo "  $id  $image  name=$name  ports=$ports  $status" >&2
        done <<< "$FOUND"
        echo "" >&2
        echo "Matched on image '$IMAGE_REF' or published host port $HOST_PORT." >&2
        echo "Re-run with --shutdown to stop it and exit, or --restart to stop it and relaunch." >&2
        exit 1
    fi
elif [ "$SHUTDOWN" = 1 ]; then
    step "Nothing to stop: no container matches image $IMAGE_REF or host port $HOST_PORT."
    exit 0
fi

# ---------------------------------------------------------------------------------------------
# 2 (second half). Validate that the required variables are actually SET.
#
# Checking only that the file exists is what lets a typo'd variable name through, and the symptom
# is a container that starts and then does not work.
# ---------------------------------------------------------------------------------------------

step "Configuration"
note "read (low to high precedence): ${SOURCED_FILES[*]}"

MISSING=()
PLACEHOLDER=()
for var in "${REQUIRED_VARS[@]}"; do
    value="${!var:-}"
    if [ -z "$value" ]; then
        MISSING+=("$var")
    elif [[ "$value" == *REPLACE-ME* ]]; then
        PLACEHOLDER+=("$var")
    fi
done

if [ "${#MISSING[@]}" -gt 0 ]; then
    die "required configuration variable(s) not set: ${MISSING[*]}

They were not found in: ${SOURCED_FILES[*]}
(nor in the environment). A file that exists but has a misspelled variable name looks fine and
produces a container that starts and then fails, which is exactly what this check is for.

Set each one in your config file. See $EXAMPLE_CONFIG for what each means."
fi

if [ "${#PLACEHOLDER[@]}" -gt 0 ]; then
    die "configuration variable(s) still hold the example placeholder: ${PLACEHOLDER[*]}
Replace the REPLACE-ME values with real ones in: ${SOURCED_FILES[*]}"
fi

# GENT_TALK_CHANNELS is comma-separated `id:label:rw` (or `:ro`). It is the most typo-prone value
# here and a malformed entry is only discovered at server startup, so check the shape now.
IFS=',' read -r -a channel_entries <<< "$GENT_TALK_CHANNELS"
for entry in "${channel_entries[@]}"; do
    [ -n "${entry// /}" ] || continue
    if ! [[ "$entry" =~ ^[[:space:]]*[0-9]+:[^:]*:(rw|ro)[[:space:]]*$ ]]; then
        die "GENT_TALK_CHANNELS entry is malformed: '$entry'
Expected comma-separated entries of the form  <channel-id>:<label>:rw  (or :ro), for example
  GENT_TALK_CHANNELS='123456789012345678:lead team:rw,987654321098765432:build noise:ro'"
    fi
done

# Report by NAME only. No value of any variable is ever printed by this script.
set_names=(); unset_names=()
for var in "${REQUIRED_VARS[@]}" "${OPTIONAL_VARS[@]}"; do
    if [ -n "${!var:-}" ]; then set_names+=("$var"); else unset_names+=("$var"); fi
done
note "set:     ${set_names[*]}"
note "not set: ${unset_names[*]:-(none)}"
note "channels configured: ${#channel_entries[@]}"

if [ -z "${GENT_TALK_ELEVENLABS_API_KEY:-}" ] || [ -z "${GENT_TALK_ELEVENLABS_AGENT_ID:-}" ]; then
    note "note: voice is OFF (needs both GENT_TALK_ELEVENLABS_API_KEY and GENT_TALK_ELEVENLABS_AGENT_ID)."
fi

# ---------------------------------------------------------------------------------------------
# 3. Tunnel check. Optional, default OFF.
# ---------------------------------------------------------------------------------------------

if [ "$TUNNEL_ENABLED" = 1 ]; then
    step "Tunnel: $TUNNEL_UNIT (systemctl --user)"
    command -v systemctl >/dev/null 2>&1 \
        || die "GENT_TALK_TUNNEL_ENABLED is on but systemctl is not on PATH.
Set GENT_TALK_TUNNEL_ENABLED=0 (or pass --no-tunnel) on a host without systemd."

    if ! systemctl --user list-unit-files --no-legend "$TUNNEL_UNIT" 2>/dev/null | grep -q . \
       && ! systemctl --user cat "$TUNNEL_UNIT" >/dev/null 2>&1; then
        die "the tunnel is enabled but the user unit '$TUNNEL_UNIT' does not exist.
Create it at ~/.config/systemd/user/$TUNNEL_UNIT (it runs 'cloudflared tunnel run <name>' and
reads ~/.cloudflared/config.yml), then:
  systemctl --user daemon-reload && systemctl --user enable --now $TUNNEL_UNIT
Or set GENT_TALK_TUNNEL_ENABLED=0 / pass --no-tunnel to skip this check."
    fi

    if systemctl --user is-active --quiet "$TUNNEL_UNIT"; then
        note "already active."
    elif [ "$DRY_RUN" = 1 ]; then
        note "(dry run) would run: systemctl --user start $TUNNEL_UNIT"
    else
        note "not active; starting it."
        systemctl --user start "$TUNNEL_UNIT" \
            || die "'systemctl --user start $TUNNEL_UNIT' failed. Diagnose with:
  systemctl --user status $TUNNEL_UNIT
  journalctl --user -u $TUNNEL_UNIT -n 50"
        systemctl --user is-active --quiet "$TUNNEL_UNIT" \
            || die "$TUNNEL_UNIT was started but is not active. See: journalctl --user -u $TUNNEL_UNIT -n 50"
        note "started."
    fi

    if ! systemctl --user is-enabled --quiet "$TUNNEL_UNIT" 2>/dev/null; then
        note "warning: $TUNNEL_UNIT is not enabled, so it will not come back after a reboot."
        note "         fix with: systemctl --user enable $TUNNEL_UNIT (and: loginctl enable-linger \"\$USER\")"
    fi
fi

# ---------------------------------------------------------------------------------------------
# 4. Rebuild.
# ---------------------------------------------------------------------------------------------

step "Building $IMAGE_REF from $GENT_TALK_DIR/Containerfile"
if [ "$DRY_RUN" = 1 ]; then
    note "(dry run) $ENGINE build -t $IMAGE_REF -f $GENT_TALK_DIR/Containerfile $GENT_TALK_DIR"
else
    "$ENGINE" build -t "$IMAGE_REF" -f "$GENT_TALK_DIR/Containerfile" "$GENT_TALK_DIR" \
        || die "image build failed. Fix the build before relaunching; the previous image is untouched."
fi

# ---------------------------------------------------------------------------------------------
# 5. Relaunch.
#
# Every secret is passed as a bare `-e NAME`, which tells the engine to take the value from THIS
# process's environment. No credential is ever written onto a command line, so none of them show
# up in `ps`, in a shell history, or in this script's own output.
# ---------------------------------------------------------------------------------------------

run_args=(run --rm -p "${HOST_ADDR}:${HOST_PORT}:8080")
for var in "${REQUIRED_VARS[@]}" "${OPTIONAL_VARS[@]}"; do
    [ -n "${!var:-}" ] || continue
    export "${var?}"
    run_args+=(-e "$var")
done
run_args+=("$IMAGE_REF")

step "Running $IMAGE_REF on ${HOST_ADDR}:${HOST_PORT}"
note "(secrets are passed by name from this process's environment, never on the command line)"
if [ "$DRY_RUN" = 1 ]; then
    note "(dry run) $ENGINE ${run_args[*]}"
    exit 0
fi
exec "$ENGINE" "${run_args[@]}"
