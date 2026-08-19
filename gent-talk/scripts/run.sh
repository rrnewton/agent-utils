#!/usr/bin/env bash
# Build and run the gent-talk container, with the checks that make a bad start fail HERE instead
# of five minutes later inside a container that came up and quietly does not work.
#
# What it does, in order:
#   1. Refuses to run if a gent-talk container is already up (see --shutdown / --restart).
#   2. Loads configuration and verifies every REQUIRED variable is actually set, by name.
#   3. Optionally (off by default) makes sure the cloudflared tunnel unit is running.
#   4. Rebuilds the image.
#   5. Removes a leftover STOPPED container, then runs the new one detached.
#
# Usage:
#   scripts/run.sh [--shutdown] [--restart] [--logs] [--follow] [--status] [--tunnel-status]
#                  [--config FILE] [--tunnel|--no-tunnel] [--tag TAG] [--port PORT]
#                  [--restart-policy POLICY] [--dry-run] [--help]
#
# Flags, one action per invocation:
#   --shutdown     Stop the running gent-talk container and EXIT (it does not go on to rebuild or
#                  relaunch). Use --restart for stop-then-relaunch. Stopping KEEPS the container
#                  and its logs; see --logs.
#   --restart      Stop the running gent-talk container, then continue with rebuild and relaunch.
#   --logs         Print the container's output SO FAR and exit — a one-shot dump. Works on a
#                  stopped container too, which is the whole point of not using --rm.
#   --follow       Stream the container's output live until you interrupt it (Ctrl-C). This is
#                  the "attach" action: a terminal tab running it follows the server exactly as
#                  though you had started the container in the foreground yourself. Nothing is
#                  stopped when you interrupt it; you are only reading.
#   --status       Report what is running, plus config, container name, restart policy, whether
#                  logs are being retained, and tunnel state; then exit.
#   --tunnel-status  Report the cloudflared tunnel user unit: whether it is active, since when,
#                  whether it is enabled at boot, and the hostname it serves. STRICTLY READ-ONLY
#                  — it never starts, stops, or enables anything. Exits 0 when the unit is
#                  active, 1 when it is not.
#   --config FILE  Use exactly this configuration file and no other. Missing file is an error.
#   --tunnel       Force the cloudflared tunnel check ON for this run.
#   --no-tunnel    Force the cloudflared tunnel check OFF for this run.
#   --tag TAG      Image tag to build and run (default: v0, or $GENT_TALK_IMAGE_TAG).
#   --port PORT    Host port to publish on (default: 8080, or $GENT_TALK_HOST_PORT).
#   --restart-policy POLICY
#                  Restart policy for the launched container (default: on-failure:5, or
#                  $GENT_TALK_RESTART_POLICY). See the discussion below before changing it.
#   --dry-run      Run every check and print the build/run commands, but build and start nothing.
#   --help         This text.
#
# How the container is run, and why:
#   Detached (-d), under a FIXED name (default: the image name, so 'gent-talk'), and WITHOUT
#   --rm. Each of those is deliberate:
#     * -d          it does not die with the terminal that launched it. Follow it with --follow.
#     * fixed name  'podman logs gent-talk' works for a human without looking anything up.
#     * no --rm     the container survives stopping, so 'podman logs' still has the per-request
#                   access log AFTER a crash — which is exactly when you want to read it. With
#                   --rm the log was destroyed by the very event you were investigating.
#   The retention guarantee, stated exactly: logs survive stopping, and are discarded at the
#   NEXT launch, when the old container is removed to make room. So look before you relaunch.
#   A leftover stopped container is removed automatically just before the relaunch, so a stale
#   one never collides with the fixed name and you never have to clean up by hand.
#
#   Detection of "is it already running?" still matches on IMAGE REFERENCE and PUBLISHED HOST
#   PORT, never on the container name. The fixed name is a convenience for humans; depending on
#   it would break the moment someone launched a container by hand, and would regress the
#   equal-length matching bug fixed in running_containers() below.
#
# Restart policy: pick it deliberately. RECOMMENDED: on-failure:5 (the default).
#   on-failure:5   Restart after a crash, at most 5 times, then stay down. A one-off crash
#                  self-heals; a crash LOOP stops and stays visibly down instead of hiding
#                  behind an endless restart, which is how an outage becomes invisible. The
#                  logs of the last attempt are still there to read.
#   always         Restart forever, including after a clean exit. Choose this only if you would
#                  rather be up-with-broken-requests than down, and you are watching --logs.
#   no             Never restart. A crash stays exactly where it fell. Most visible, least
#                  available; a fine choice while debugging.
#   unless-stopped On PODMAN this is IDENTICAL to 'always' (podman-run(1) says so) — it is NOT
#                  the docker behaviour some expect.
#   Surviving a REBOOT is a separate switch, and no policy value alone provides it: podman only
#   restarts containers across a reboot via podman-restart.service, which starts containers
#   whose policy is literally 'always'. To get reboot survival:
#     systemctl --user enable --now podman-restart.service   # and: loginctl enable-linger $USER
#   and run with --restart-policy always. Until you do that, the tunnel unit comes back after a
#   reboot and gent-talk does not — which --tunnel-status and --status will show you.
#
# Configuration precedence, HIGHEST first:
#   1. Variables already exported in your shell environment. These always win.
#   2. --config FILE, if given. When passed, no other file is read.
#   3. ~/.config/gent-talk/env         — per-user base, shared by every checkout on this machine.
# There is exactly ONE default configuration file, so there is never a question of which copy is
# live. Copy gent-talk/gent-talk.env.example to it to get started; it documents every variable.
#
# This script never prints the value of a credential. It reports variables by NAME only.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
GENT_TALK_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"

USER_CONFIG_DEFAULT="${XDG_CONFIG_HOME:-$HOME/.config}/gent-talk/env"
USER_CONFIG_DIR="$(dirname -- "$USER_CONFIG_DEFAULT")"
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
    GENT_TALK_CONTAINER_NAME
    GENT_TALK_RESTART_POLICY
    GENT_TALK_TUNNEL_ENABLED
    GENT_TALK_TUNNEL_UNIT
    GENT_TALK_TUNNEL_CONFIG
    GENT_TALK_ENGINE
)

CONFIG_FILE=""
SHUTDOWN=0
RESTART=0
DRY_RUN=0
STATUS_ONLY=0
LOGS_ONLY=0
FOLLOW=0
TUNNEL_STATUS_ONLY=0
TUNNEL_OVERRIDE=""
TAG_OVERRIDE=""
PORT_OVERRIDE=""
POLICY_OVERRIDE=""

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
        --restart-policy) POLICY_OVERRIDE="${2:?--restart-policy needs a value}"; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        --status) STATUS_ONLY=1; shift ;;
        --logs) LOGS_ONLY=1; shift ;;
        --follow) FOLLOW=1; shift ;;
        --tunnel-status) TUNNEL_STATUS_ONLY=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unrecognized argument: $1" >&2; echo "" >&2; usage >&2; exit 2 ;;
    esac
done

if [ "$SHUTDOWN" = 1 ] && [ "$RESTART" = 1 ]; then
    die "--shutdown and --restart contradict each other. --shutdown stops and exits; --restart stops and then relaunches."
fi

# Every one of these ends the run in a different place, so asking for two of them at once is
# always a mistake, and a silently-ignored flag is the worst way to answer it.
# (Written as `if` blocks, not `[ ... ] && ...`: under `set -e` a trailing false test on the last
# such line would exit the script.)
ACTIONS_GIVEN=()
if [ "$SHUTDOWN" = 1 ]; then ACTIONS_GIVEN+=(--shutdown); fi
if [ "$RESTART" = 1 ]; then ACTIONS_GIVEN+=(--restart); fi
if [ "$STATUS_ONLY" = 1 ]; then ACTIONS_GIVEN+=(--status); fi
if [ "$LOGS_ONLY" = 1 ]; then ACTIONS_GIVEN+=(--logs); fi
if [ "$FOLLOW" = 1 ]; then ACTIONS_GIVEN+=(--follow); fi
if [ "$TUNNEL_STATUS_ONLY" = 1 ]; then ACTIONS_GIVEN+=(--tunnel-status); fi
if [ "${#ACTIONS_GIVEN[@]}" -gt 1 ]; then
    die "these actions cannot be combined: ${ACTIONS_GIVEN[*]}
Each one ends the run somewhere different. Pick exactly one and re-run.
  --logs dumps output and exits;  --follow streams it;  --status and --tunnel-status report;
  --shutdown stops and exits;     --restart stops and relaunches."
fi

# ---------------------------------------------------------------------------------------------
# 2 (first half). Load configuration.
#
# Precedence is implemented by remembering which variables the CALLING environment already set,
# sourcing the config file, then restoring the caller's values on top. That is what makes an
# exported variable beat the file without the file having to know about it.
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
fi

for var in "${!PRESET_VALUES[@]}"; do
    printf -v "$var" '%s' "${PRESET_VALUES[$var]}"
done

if [ "${#SOURCED_FILES[@]}" -eq 0 ]; then
    die "no configuration file found.

Looked for:
  $USER_CONFIG_DEFAULT

Create it with:
  mkdir -p $USER_CONFIG_DIR
  cp $EXAMPLE_CONFIG $USER_CONFIG_DEFAULT
  \$EDITOR $USER_CONFIG_DEFAULT

That example documents every variable and marks which are required. That file holds credentials:
it lives outside any git checkout on purpose, so keep it there and readable only by you."
fi

# ---------------------------------------------------------------------------------------------
# Launcher settings, after config load so the config file can set them.
# ---------------------------------------------------------------------------------------------

IMAGE_NAME="${GENT_TALK_IMAGE_NAME:-gent-talk}"
IMAGE_TAG="${TAG_OVERRIDE:-${GENT_TALK_IMAGE_TAG:-v0}}"
HOST_PORT="${PORT_OVERRIDE:-${GENT_TALK_HOST_PORT:-8080}}"
HOST_ADDR="${GENT_TALK_HOST_ADDR:-127.0.0.1}"
TUNNEL_UNIT="${GENT_TALK_TUNNEL_UNIT:-cloudflared-gent-talk.service}"
TUNNEL_CONFIG="${GENT_TALK_TUNNEL_CONFIG:-$HOME/.cloudflared/config.yml}"
ENGINE="${GENT_TALK_ENGINE:-podman}"
IMAGE_REF="${IMAGE_NAME}:${IMAGE_TAG}"
# Defaults to the image name, so the self-test's throwaway image gets a throwaway container name
# and can never collide with the real 'gent-talk'.
CONTAINER_NAME="${GENT_TALK_CONTAINER_NAME:-$IMAGE_NAME}"
RESTART_POLICY="${POLICY_OVERRIDE:-${GENT_TALK_RESTART_POLICY:-on-failure:5}}"

case "$HOST_PORT" in
    ''|*[!0-9]*) die "host port must be a number, got: $HOST_PORT" ;;
esac

# Container names are [a-zA-Z0-9][a-zA-Z0-9_.-]*; anything else is rejected by the engine with a
# message that does not mention this script, minutes into a launch.
if ! [[ "$CONTAINER_NAME" =~ ^[a-zA-Z0-9][a-zA-Z0-9_.-]*$ ]]; then
    die "container name is not usable: '$CONTAINER_NAME'
It must start with a letter or digit and contain only letters, digits, '_', '.' or '-'.
Set GENT_TALK_CONTAINER_NAME (or GENT_TALK_IMAGE_NAME, which it defaults to)."
fi

# Checked here rather than at `run` time: an unknown policy is otherwise discovered AFTER the
# image has been rebuilt and the old container stopped.
if ! [[ "$RESTART_POLICY" =~ ^(no|never|always|unless-stopped|on-failure(:[0-9]+)?)$ ]]; then
    die "restart policy is not one podman accepts: '$RESTART_POLICY'
Valid: no, never, on-failure, on-failure:<max-retries>, always, unless-stopped.
Recommended: on-failure:5 — see --help for why, and for what it takes to survive a reboot."
fi

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

matching_containers() {  # matching_containers [extra `ps` flags, e.g. -a]
    "$ENGINE" ps "$@" --format '{{.ID}}|{{.Image}}|{{.Names}}|{{.Ports}}|{{.Status}}' 2>/dev/null \
        | awk -F'|' -v img="$IMAGE_REF" -v port="$HOST_PORT" '
            {
                # A registry-qualified image reads "localhost/gent-talk:v0", so accept an exact
                # match or a "/"-prefixed suffix match. The index() > 0 guard is load-bearing:
                # index() returns 0 when the substring is ABSENT, and 0 also equals
                # length($2) - length(img) whenever the two references happen to be the same
                # length -- so without it, "gent-talk:dryrun-probe" matched the unrelated
                # "localhost/gent-talk:ci". A false match here is not cosmetic: under --shutdown
                # or --restart this function decides what gets STOPPED.
                slash_at = index($2, "/" img)
                image_match = ($2 == img) || (slash_at > 0 && slash_at == length($2) - length(img))
                # A published port renders as e.g. "127.0.0.1:8080->8080/tcp"; requiring the
                # colon immediately before the number keeps ":8080->" from matching ":18080->".
                port_match = (index($4, ":" port "->") > 0)
                if (image_match || port_match) print $0
            }'
}

running_containers() { matching_containers; }
# `ps -a` also lists stopped ones. Podman still reports their published ports there, so the same
# image-or-port rule applies unchanged.
all_containers() { matching_containers -a; }

# The stopped ones are "all, minus the running ones", compared by container ID. Deriving it that
# way rather than by pattern-matching the Status column means no wording of "Exited (137) ..." or
# "Created" can be misread as running.
stopped_containers() {
    local running_ids line id
    running_ids="$(running_containers | cut -d'|' -f1)"
    all_containers | while IFS= read -r line; do
        id="${line%%|*}"
        [ -n "$id" ] || continue
        if ! grep -qxF -- "$id" <<< "$running_ids"; then printf '%s\n' "$line"; fi
    done
}

# Reports "<policy>|<autoremove>" for one container, e.g. "on-failure:5|false".
container_run_mode() {
    local id="$1" raw name count autoremove
    raw="$("$ENGINE" inspect --format \
        '{{.HostConfig.RestartPolicy.Name}}|{{.HostConfig.RestartPolicy.MaximumRetryCount}}|{{.HostConfig.AutoRemove}}' \
        "$id" 2>/dev/null)" || raw=""
    if [ -z "$raw" ]; then echo "unknown|unknown"; return 0; fi
    IFS='|' read -r name count autoremove <<< "$raw"
    [ -n "$name" ] || name="(none)"
    if [ "$name" = "on-failure" ] && [ -n "$count" ] && [ "$count" != 0 ]; then name="on-failure:$count"; fi
    echo "${name}|${autoremove}"
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

remove_stopped() {
    local found="$1" id name
    while IFS='|' read -r id _image name _ports _status; do
        [ -n "$id" ] || continue
        step "Removing stopped container $name ($id) to free the name and the port"
        note "its logs go with it — read them first with --logs if you still need them."
        if [ "$DRY_RUN" = 1 ]; then
            note "(dry run) $ENGINE rm $id"
        else
            "$ENGINE" rm "$id" >/dev/null \
                || die "failed to remove stopped container $id ($name). Remove it by hand: $ENGINE rm $id"
        fi
    done <<< "$found"
}

# --------------------------------------------------------------------------------------------
# Tunnel status. STRICTLY READ-ONLY: it queries systemd and reads the cloudflared config, and
# starts, stops and enables nothing. Returns 0 only when the unit is active.
#
# Of the cloudflared config it prints ONLY ingress hostnames. The same file holds the tunnel id
# and the path of the credentials file, and neither belongs in this script's output.
# --------------------------------------------------------------------------------------------

tunnel_hostnames() {
    [ -r "$TUNNEL_CONFIG" ] || return 1
    grep -E '^[[:space:]]*-?[[:space:]]*hostname:[[:space:]]*' "$TUNNEL_CONFIG" 2>/dev/null \
        | sed -E 's/^[[:space:]]*-?[[:space:]]*hostname:[[:space:]]*//; s/[[:space:]]+$//; s/^"(.*)"$/\1/; s/^'"'"'(.*)'"'"'$/\1/'
}

tunnel_status_report() {
    step "Tunnel status: $TUNNEL_UNIT (systemctl --user)"
    note "read-only: this action starts, stops and enables nothing."

    if ! command -v systemctl >/dev/null 2>&1; then
        note "unit:      CANNOT TELL — systemctl is not on PATH (not a systemd host)."
        return 1
    fi

    local load active sub since enabled
    load="$(systemctl --user show "$TUNNEL_UNIT" -p LoadState --value 2>/dev/null || true)"
    if [ "$load" != "loaded" ]; then
        note "unit:      NOT INSTALLED (systemd LoadState=${load:-unknown})"
        note "expected:  ~/.config/systemd/user/$TUNNEL_UNIT"
        note "install:   write that unit (it runs 'cloudflared tunnel run <name>' and reads"
        note "           ~/.cloudflared/config.yml), then:"
        note "             systemctl --user daemon-reload"
        note "             systemctl --user enable --now $TUNNEL_UNIT"
        return 1
    fi

    active="$(systemctl --user show "$TUNNEL_UNIT" -p ActiveState --value 2>/dev/null || true)"
    sub="$(systemctl --user show "$TUNNEL_UNIT" -p SubState --value 2>/dev/null || true)"
    since="$(systemctl --user show "$TUNNEL_UNIT" -p ActiveEnterTimestamp --value 2>/dev/null || true)"
    enabled="$(systemctl --user is-enabled "$TUNNEL_UNIT" 2>/dev/null || true)"

    note "unit:      installed"
    if [ "$active" = "active" ]; then
        note "active:    YES (ActiveState=$active, SubState=$sub)"
        note "since:     ${since:-unknown}"
    else
        note "active:    NO (ActiveState=${active:-unknown}, SubState=${sub:-unknown})"
        note "start it:  systemctl --user start $TUNNEL_UNIT"
        note "diagnose:  journalctl --user -u $TUNNEL_UNIT -n 50"
    fi
    note "at boot:   ${enabled:-unknown}$([ "$enabled" = enabled ] && echo "" || echo "  (it will NOT come back after a reboot; fix with: systemctl --user enable $TUNNEL_UNIT && loginctl enable-linger \"\$USER\")")"

    local hosts
    hosts="$(tunnel_hostnames || true)"
    if [ -n "$hosts" ]; then
        note "hostname:  $(printf '%s' "$hosts" | paste -sd', ' -)"
        note "           (ingress hostnames from $TUNNEL_CONFIG)"
    else
        note "hostname:  unknown — no ingress hostname readable in $TUNNEL_CONFIG"
        note "           (set GENT_TALK_TUNNEL_CONFIG if the tunnel config lives elsewhere)"
    fi
    note "origin:    the tunnel is configured independently of this script; it points at"
    note "           whatever host port its own ingress says, normally ${HOST_ADDR}:${HOST_PORT}."

    [ "$active" = "active" ]
}

if [ "$TUNNEL_STATUS_ONLY" = 1 ]; then
    rc=0
    tunnel_status_report || rc=$?
    exit "$rc"
fi

FOUND="$(running_containers || true)"

if [ "$STATUS_ONLY" = 1 ]; then
    STOPPED="$(stopped_containers || true)"
    step "gent-talk status"
    note "image:          $IMAGE_REF"
    note "publish:        ${HOST_ADDR}:${HOST_PORT}"
    note "container name: $CONTAINER_NAME  (fixed, so '$ENGINE logs $CONTAINER_NAME' just works)"
    note "restart policy: $RESTART_POLICY  (applied at the next launch)"
    note "logs:           RETAINED after the container stops (it is not run with --rm)."
    note "                read them with: $SCRIPT_DIR/run.sh --logs   (or --follow to stream)"
    note "                they are discarded at the next launch, when the old container is removed."
    note "config file:    ${SOURCED_FILES[*]}"
    note "tunnel check:   $([ "$TUNNEL_ENABLED" = 1 ] && echo "enabled (unit $TUNNEL_UNIT)" || echo "disabled")   — see --tunnel-status for the unit itself"
    if [ -n "$FOUND" ]; then
        note "running:"
        while IFS='|' read -r id image name ports status; do
            [ -n "$id" ] || continue
            note "  $id  $image  name=$name  ports=$ports  $status"
            mode="$(container_run_mode "$id")"
            note "      restart-policy=${mode%%|*}  autoremove=${mode##*|}"
            if [ "${mode##*|}" = "true" ]; then
                note "      NOTE: this one was started the OLD way (--rm): its logs are destroyed the"
                note "            moment it stops, so a crash takes its own evidence with it. Cut over"
                note "            with: $SCRIPT_DIR/run.sh --restart"
            fi
        done <<< "$FOUND"
    else
        note "running:        nothing matching $IMAGE_REF or host port $HOST_PORT"
    fi
    # Reboot survival is NOT a property of the restart policy on its own (see --help): podman
    # restores containers across a reboot only through podman-restart.service, and that unit
    # starts only containers whose policy is literally 'always'. Reporting the two facts
    # separately is the point — either one alone reads as "it will come back" when it will not.
    prs_state="unknown (no systemctl)"
    if command -v systemctl >/dev/null 2>&1; then
        prs_state="$(systemctl --user is-enabled podman-restart.service 2>/dev/null || echo disabled)"
    fi
    note "after a reboot: podman-restart.service is $prs_state, and it restores ONLY containers"
    note "                whose restart policy is literally 'always'. The cloudflared tunnel is a"
    note "                separate unit with its own answer — see --tunnel-status."
    if [ -n "$STOPPED" ]; then
        note "stopped (kept for their logs; removed at the next launch):"
        while IFS='|' read -r id image name ports status; do
            [ -n "$id" ] || continue
            note "  $id  $image  name=$name  ports=$ports  $status"
        done <<< "$STOPPED"
    fi
    exit 0
fi

# --------------------------------------------------------------------------------------------
# Logs. A stopped container still has them, which is the reason --rm is not used.
# --------------------------------------------------------------------------------------------

if [ "$LOGS_ONLY" = 1 ] || [ "$FOLLOW" = 1 ]; then
    target=""
    if [ -n "$FOUND" ]; then
        target="$(head -1 <<< "$FOUND")"
    else
        target="$(all_containers | head -1 || true)"
    fi
    if [ -z "${target%%|*}" ] || [ -z "$target" ]; then
        die "no gent-talk container to read logs from.
Nothing matches image '$IMAGE_REF' or published host port $HOST_PORT, running or stopped.

If it was started before this script grew a fixed name, it may be under a different image tag:
  $ENGINE ps -a
Otherwise start it with: $SCRIPT_DIR/run.sh"
    fi
    IFS='|' read -r log_id _log_image log_name _log_ports log_status <<< "$target"
    if [ "$FOLLOW" = 1 ]; then
        step "Following $log_name ($log_id) — $log_status. Ctrl-C stops READING, not the container."
        if [ "$DRY_RUN" = 1 ]; then note "(dry run) $ENGINE logs -f $log_id"; exit 0; fi
        exec "$ENGINE" logs -f "$log_id"
    fi
    step "Logs for $log_name ($log_id) — $log_status"
    if [ "$DRY_RUN" = 1 ]; then note "(dry run) $ENGINE logs $log_id"; exit 0; fi
    exec "$ENGINE" logs "$log_id"
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
note "config file: ${SOURCED_FILES[*]}"

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

# The leftover-container sweep happens HERE, as late as possible — after the build has succeeded
# — and not up in the detection section. Removing a stopped container destroys its logs, so if
# the run is going to abort for a bad config or a failed build, the evidence from the previous
# instance is still there to read.
STOPPED="$(stopped_containers || true)"
if [ -n "$STOPPED" ]; then
    remove_stopped "$STOPPED"
fi

# Narrow safety net, not the detection rule: a container could hold the fixed name without
# matching our image or port (someone launched one by hand, or the image was renamed). Without
# this, `run --name` fails with an engine-level error that says nothing about this script.
NAME_HOLDER="$("$ENGINE" ps -a --filter "name=^${CONTAINER_NAME}$" --format '{{.ID}}|{{.Image}}|{{.Status}}' 2>/dev/null | head -1 || true)"
if [ -n "$NAME_HOLDER" ]; then
    holder_id="${NAME_HOLDER%%|*}"
    if "$ENGINE" ps --filter "name=^${CONTAINER_NAME}$" --format '{{.ID}}' 2>/dev/null | grep -qxF -- "$holder_id"; then
        die "the name '$CONTAINER_NAME' is held by a RUNNING container that does not match
image '$IMAGE_REF' or host port $HOST_PORT:
  $NAME_HOLDER
Refusing to touch it — it is not ours to stop. Stop it yourself, or set
GENT_TALK_CONTAINER_NAME to a different name."
    fi
    step "Removing stopped container '$CONTAINER_NAME' ($holder_id) that holds the name"
    if [ "$DRY_RUN" = 1 ]; then
        note "(dry run) $ENGINE rm $holder_id"
    else
        "$ENGINE" rm "$holder_id" >/dev/null \
            || die "failed to remove '$CONTAINER_NAME' ($holder_id). Remove it by hand: $ENGINE rm $holder_id"
    fi
fi

# -d and no --rm: see the header. The container outlives this shell, and keeps its logs when it
# stops.
run_args=(run -d --name "$CONTAINER_NAME" --restart "$RESTART_POLICY" -p "${HOST_ADDR}:${HOST_PORT}:8080")
for var in "${REQUIRED_VARS[@]}" "${OPTIONAL_VARS[@]}"; do
    [ -n "${!var:-}" ] || continue
    export "${var?}"
    run_args+=(-e "$var")
done
run_args+=("$IMAGE_REF")

step "Running $IMAGE_REF as '$CONTAINER_NAME' on ${HOST_ADDR}:${HOST_PORT} (detached, restart=$RESTART_POLICY)"
note "(secrets are passed by name from this process's environment, never on the command line)"
if [ "$DRY_RUN" = 1 ]; then
    note "(dry run) $ENGINE ${run_args[*]}"
    exit 0
fi

CONTAINER_ID="$("$ENGINE" "${run_args[@]}")" \
    || die "the container failed to start. The image built fine, so this is a runtime problem:
  $ENGINE logs $CONTAINER_NAME"

step "Started $CONTAINER_NAME (${CONTAINER_ID:0:12})"
note "follow the output:  $SCRIPT_DIR/run.sh --follow"
note "one-shot log dump:  $SCRIPT_DIR/run.sh --logs"
note "stop it:            $SCRIPT_DIR/run.sh --shutdown   (logs survive; they go at the next launch)"
note "state, any time:    $SCRIPT_DIR/run.sh --status"
