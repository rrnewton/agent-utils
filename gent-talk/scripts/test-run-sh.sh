#!/usr/bin/env bash
# Tests for scripts/run.sh. Safe to run on a host where the real gent-talk is live: every check
# uses a throwaway image name and a throwaway port, and the only container it ever creates or
# stops is its own. It never builds a gent-talk image and never touches port 8080.
#
#   scripts/test-run-sh.sh
#
# The container-detection checks need podman (or $GENT_TALK_ENGINE); they are skipped, loudly,
# if it is not available.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RUN_SH="$SCRIPT_DIR/run.sh"
ENGINE="${GENT_TALK_ENGINE:-podman}"

# Deliberately distinctive so the no-leak check below cannot pass by accident.
SENTINEL_BOT_TOKEN="SENTINEL-BOT-TOKEN-9d41f0c2"
SENTINEL_READ_TOKEN="SENTINEL-READ-TOKEN-7b3ea15d"
SENTINEL_WRITE_TOKEN="SENTINEL-WRITE-TOKEN-2c98da44"
SENTINEL_EL_KEY="SENTINEL-ELEVENLABS-KEY-51ab7e30"

TEST_IMAGE_NAME="gent-talk-selftest"
TEST_IMAGE_TAG="t0"
TEST_PORT=18099

TMPDIR_TEST="$(mktemp -d)"
ALL_OUTPUT="$TMPDIR_TEST/all-output.txt"
: > "$ALL_OUTPUT"
CREATED_CONTAINER=""

cleanup() {
    if [ -n "$CREATED_CONTAINER" ]; then
        "$ENGINE" rm -f "$CREATED_CONTAINER" >/dev/null 2>&1 || true
    fi
    "$ENGINE" rmi "${TEST_IMAGE_NAME}:${TEST_IMAGE_TAG}" >/dev/null 2>&1 || true
    rm -rf "$TMPDIR_TEST"
}
trap cleanup EXIT

PASS=0
fail() { echo "FAILED: $*" >&2; exit 1; }
ok() { PASS=$((PASS + 1)); echo "ok   $*"; }

# Runs run.sh in a scrubbed environment (`env -i`), so no GENT_TALK_* variable exported in this
# shell can leak into a test, and records every byte of output for the credential-leak check.
#
# HOME, XDG_RUNTIME_DIR and DBUS stay REAL on purpose: rootless podman and `systemctl --user`
# resolve their state through them. In particular, pointing XDG_CONFIG_HOME at a temp directory
# makes podman read a different, empty configuration and report ZERO running containers without
# any error — which would turn the already-running checks below into silent false passes. Tests
# that must not see the real ~/.config/gent-talk/env therefore either pass --config (which
# replaces the file layer) or use run_sh_isolated_config, which does not need podman.
#
# That trap is why the tunnel-from-user-config checks below assert on run.sh's OWN error text
# rather than on anything podman reports.
TEST_XDG_CONFIG="$TMPDIR_TEST/config"
run_sh() {
    local out rc=0
    out="$(env -i \
            PATH="$PATH" \
            HOME="$HOME" \
            USER="${USER:-}" \
            XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-}" \
            DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-}" \
            GENT_TALK_ENGINE="$ENGINE" \
            bash "$RUN_SH" "$@" 2>&1)" || rc=$?
    printf '%s\n' "$out" >> "$ALL_OUTPUT"
    printf '%s' "$out"
    return "$rc"
}

# Same, but with the per-user config directory redirected at a temp tree, for the checks about
# WHICH config file is read. Container detection is not meaningful under this one (see above).
run_sh_isolated_config() {
    local out rc=0
    out="$(env -i \
            PATH="$PATH" \
            HOME="$HOME" \
            USER="${USER:-}" \
            XDG_CONFIG_HOME="$TEST_XDG_CONFIG" \
            XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-}" \
            DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-}" \
            GENT_TALK_ENGINE="$ENGINE" \
            bash "$RUN_SH" "$@" 2>&1)" || rc=$?
    printf '%s\n' "$out" >> "$ALL_OUTPUT"
    printf '%s' "$out"
    return "$rc"
}

mkdir -p "$TMPDIR_TEST/home" "$TEST_XDG_CONFIG"

write_config() {  # write_config FILE [omit-var]
    local file="$1" omit="${2:-}"
    : > "$file"
    [ "$omit" = GENT_TALK_DISCORD_BOT_TOKEN ] || echo "GENT_TALK_DISCORD_BOT_TOKEN=$SENTINEL_BOT_TOKEN" >> "$file"
    [ "$omit" = GENT_TALK_READ_TOKEN ] || echo "GENT_TALK_READ_TOKEN=$SENTINEL_READ_TOKEN" >> "$file"
    [ "$omit" = GENT_TALK_WRITE_TOKEN ] || echo "GENT_TALK_WRITE_TOKEN=$SENTINEL_WRITE_TOKEN" >> "$file"
    [ "$omit" = GENT_TALK_CHANNELS ] || echo "GENT_TALK_CHANNELS='123456789012345678:lead team:rw,987654321098765432:build noise:ro'" >> "$file"
    echo "GENT_TALK_ELEVENLABS_API_KEY=$SENTINEL_EL_KEY" >> "$file"
    echo "GENT_TALK_IMAGE_NAME=$TEST_IMAGE_NAME" >> "$file"
    echo "GENT_TALK_IMAGE_TAG=$TEST_IMAGE_TAG" >> "$file"
    echo "GENT_TALK_HOST_PORT=$TEST_PORT" >> "$file"
}

GOOD_CONFIG="$TMPDIR_TEST/good.env"
write_config "$GOOD_CONFIG"

echo "== run.sh tests =="

# --- 1. --help ------------------------------------------------------------------------------
out="$(run_sh --help)"
grep -q -- '--shutdown' <<< "$out" || fail "--help does not document --shutdown"
grep -q 'Configuration precedence' <<< "$out" || fail "--help does not explain config precedence"
grep -q 'EXIT' <<< "$out" || fail "--help does not say that --shutdown exits"
ok "--help documents the flags and the config precedence"

# --- 2. no configuration at all -------------------------------------------------------------
# Run a COPY of run.sh from a temp directory, with an empty HOME and XDG_CONFIG_HOME, so the
# real ~/.config/gent-talk/env is out of scope and no config file exists at all.
mkdir -p "$TMPDIR_TEST/isolated/scripts"
cp "$RUN_SH" "$TMPDIR_TEST/isolated/scripts/run.sh"
rc=0
out="$(env -i PATH="$PATH" HOME="$TMPDIR_TEST/home-empty" \
        XDG_CONFIG_HOME="$TMPDIR_TEST/config-empty" GENT_TALK_ENGINE="$ENGINE" \
        bash "$TMPDIR_TEST/isolated/scripts/run.sh" --dry-run 2>&1)" || rc=$?
printf '%s\n' "$out" >> "$ALL_OUTPUT"
[ "$rc" -ne 0 ] || fail "missing config did not fail"
grep -q 'no configuration file found' <<< "$out" || fail "missing config error is not specific"
grep -q 'gent-talk.env.example' <<< "$out" || fail "missing config error does not name the example file"
ok "missing configuration fails, naming the paths searched and the example to copy"

# --- 2b. a file at the REMOVED per-checkout path is not read --------------------------------
# gent-talk/.gent-talk.env used to be a config layer. It is not one any more. A complete, valid
# config sitting at that exact path next to run.sh must therefore change nothing: the run still
# fails for want of a config file. Asserting the failure (rather than just "it launched") is what
# makes this check impossible to pass by accident.
write_config "$TMPDIR_TEST/isolated/.gent-talk.env"
rc=0
out="$(env -i PATH="$PATH" HOME="$TMPDIR_TEST/home-empty" \
        XDG_CONFIG_HOME="$TMPDIR_TEST/config-empty" GENT_TALK_ENGINE="$ENGINE" \
        bash "$TMPDIR_TEST/isolated/scripts/run.sh" --dry-run 2>&1)" || rc=$?
printf '%s\n' "$out" >> "$ALL_OUTPUT"
[ "$rc" -ne 0 ] || fail "a config at the removed gent-talk/.gent-talk.env path was still honoured"
grep -q 'no configuration file found' <<< "$out" || fail "removed-overlay run failed for some other reason: $out"
grep -q '\.gent-talk\.env' <<< "$out" && fail "run.sh still mentions the removed .gent-talk.env path"
rm -f "$TMPDIR_TEST/isolated/.gent-talk.env"
ok "a config file at the removed per-checkout path is ignored, and never mentioned"

# --- 3. --config pointing at a file that is not there ----------------------------------------
rc=0; out="$(run_sh --config "$TMPDIR_TEST/nope.env" --dry-run)" || rc=$?
[ "$rc" -ne 0 ] || fail "--config with a missing file did not fail"
grep -q 'not found' <<< "$out" || fail "--config missing-file error is not specific"
ok "--config with a missing file fails, naming the path"

# --- 4. a config that exists but is missing a required variable ------------------------------
for missing in GENT_TALK_WRITE_TOKEN GENT_TALK_CHANNELS; do
    bad="$TMPDIR_TEST/missing-$missing.env"
    write_config "$bad" "$missing"
    rc=0; out="$(run_sh --config "$bad" --dry-run)" || rc=$?
    [ "$rc" -ne 0 ] || fail "config missing $missing did not fail"
    grep -q "$missing" <<< "$out" || fail "error for missing $missing does not name the variable"
    ok "config missing $missing fails, naming that variable"
done

# --- 5. placeholder values still in place ----------------------------------------------------
ph="$TMPDIR_TEST/placeholder.env"
write_config "$ph"
sed -i "s/^GENT_TALK_READ_TOKEN=.*/GENT_TALK_READ_TOKEN=REPLACE-ME-at-least-24-characters/" "$ph"
rc=0; out="$(run_sh --config "$ph" --dry-run)" || rc=$?
[ "$rc" -ne 0 ] || fail "unedited placeholder value did not fail"
grep -q 'GENT_TALK_READ_TOKEN' <<< "$out" || fail "placeholder error does not name the variable"
ok "an unedited REPLACE-ME value fails, naming that variable"

# --- 6. malformed channel list ---------------------------------------------------------------
badch="$TMPDIR_TEST/badchannels.env"
write_config "$badch"
sed -i "s/^GENT_TALK_CHANNELS=.*/GENT_TALK_CHANNELS='123456789012345678:lead team'/" "$badch"
rc=0; out="$(run_sh --config "$badch" --dry-run)" || rc=$?
[ "$rc" -ne 0 ] || fail "malformed GENT_TALK_CHANNELS did not fail"
grep -q 'GENT_TALK_CHANNELS entry is malformed' <<< "$out" || fail "malformed channels error is not specific"
ok "a malformed GENT_TALK_CHANNELS entry fails before anything is built"

# --- 7. the per-user config location is the one that works ------------------------------------
mkdir -p "$TEST_XDG_CONFIG/gent-talk"
cp "$GOOD_CONFIG" "$TEST_XDG_CONFIG/gent-talk/env"
out="$(run_sh_isolated_config --dry-run --no-tunnel)"
grep -q 'Running' <<< "$out" || fail "user config did not reach the run step"
grep -q "config file: $TEST_XDG_CONFIG/gent-talk/env" <<< "$out" \
    || fail "run.sh did not report reading the per-user config file"
ok "~/.config/gent-talk/env alone is enough (the owner's current setup keeps working)"

# --- 7b. GENT_TALK_TUNNEL_ENABLED is honoured FROM the per-user config file -------------------
# This flag lived only in the deleted per-checkout overlay, and losing it silently turned the
# tunnel check off. The per-user file is now the only place it can live, so prove it is read
# from there: not from the environment, not from --config, and not from a flag on the command
# line. The proof is that run.sh reaches the tunnel step and fails on a unit that cannot exist.
TUNNEL_CONFIG="$TEST_XDG_CONFIG/gent-talk/env"
cp "$GOOD_CONFIG" "$TUNNEL_CONFIG"
echo "GENT_TALK_TUNNEL_ENABLED=1" >> "$TUNNEL_CONFIG"
echo "GENT_TALK_TUNNEL_UNIT=cloudflared-gent-talk-not-a-real-unit.service" >> "$TUNNEL_CONFIG"
rc=0; out="$(run_sh_isolated_config --dry-run)" || rc=$?
[ "$rc" -ne 0 ] || fail "GENT_TALK_TUNNEL_ENABLED=1 in the per-user config did not enable the tunnel check"
grep -q 'cloudflared-gent-talk-not-a-real-unit.service' <<< "$out" \
    || fail "tunnel check ran but did not name the unit from the per-user config"
ok "GENT_TALK_TUNNEL_ENABLED=1 in ~/.config/gent-talk/env turns the tunnel check ON"

# Negative control: same file, flag flipped to 0. Without this, the check above would still pass
# if run.sh had started failing at the tunnel step for some reason unrelated to the flag.
sed -i "s/^GENT_TALK_TUNNEL_ENABLED=.*/GENT_TALK_TUNNEL_ENABLED=0/" "$TUNNEL_CONFIG"
out="$(run_sh_isolated_config --dry-run)"
grep -q 'Running' <<< "$out" || fail "tunnel flag 0 in the per-user config should have reached the run step"
grep -q 'systemctl' <<< "$out" && fail "tunnel flag 0 but the tunnel check still ran"
ok "GENT_TALK_TUNNEL_ENABLED=0 in the same file turns it back OFF (the flag is what decides)"

cp "$GOOD_CONFIG" "$TUNNEL_CONFIG"

# --- 8. a good config reaches build and run, in dry run --------------------------------------
out="$(run_sh --config "$GOOD_CONFIG" --dry-run --no-tunnel)"
grep -q '(dry run).*build -t '"$TEST_IMAGE_NAME:$TEST_IMAGE_TAG" <<< "$out" || fail "dry run did not print the build command"
grep -q '(dry run).*run --rm -p 127.0.0.1:'"$TEST_PORT" <<< "$out" || fail "dry run did not print the run command"
grep -q 'set: .*GENT_TALK_DISCORD_BOT_TOKEN' <<< "$out" || fail "dry run does not report which variables are set"
ok "a valid config reaches build and run (dry run), reporting variables by name"

# --- 9. tunnel check, both settings ----------------------------------------------------------
out="$(run_sh --config "$GOOD_CONFIG" --dry-run --no-tunnel)"
grep -q 'systemctl' <<< "$out" && fail "tunnel disabled but systemd was still mentioned"
ok "tunnel disabled: skipped silently"

nounit="$TMPDIR_TEST/nounit.env"
write_config "$nounit"
echo "GENT_TALK_TUNNEL_UNIT=cloudflared-gent-talk-does-not-exist.service" >> "$nounit"
rc=0; out="$(run_sh --config "$nounit" --dry-run --tunnel)" || rc=$?
[ "$rc" -ne 0 ] || fail "tunnel enabled with a nonexistent unit did not fail"
grep -q 'cloudflared-gent-talk-does-not-exist.service' <<< "$out" || fail "nonexistent-unit error does not name the unit"
grep -qi 'does not exist' <<< "$out" || fail "nonexistent-unit error is not a clear message"
ok "tunnel enabled with a missing unit: clear error naming the unit, not a crash"

# --- 10. already-running detection, by image and by port -------------------------------------
if command -v "$ENGINE" >/dev/null 2>&1; then
    BASE_IMAGE="$("$ENGINE" images --format '{{.Repository}}:{{.Tag}}' | grep -E '^docker.io/library/debian:' | head -1 || true)"
    [ -n "$BASE_IMAGE" ] || BASE_IMAGE="$("$ENGINE" images --format '{{.Repository}}:{{.Tag}}' | grep -v '<none>' | head -1 || true)"
    if [ -z "$BASE_IMAGE" ]; then
        echo "SKIP no local image to tag for the detection test" >&2
    else
        "$ENGINE" tag "$BASE_IMAGE" "${TEST_IMAGE_NAME}:${TEST_IMAGE_TAG}" >/dev/null
        CREATED_CONTAINER="$("$ENGINE" run -d -p "127.0.0.1:${TEST_PORT}:8080" \
            "${TEST_IMAGE_NAME}:${TEST_IMAGE_TAG}" sleep 600)"
        sleep 1

        rc=0; out="$(run_sh --config "$GOOD_CONFIG" --dry-run --no-tunnel)" || rc=$?
        [ "$rc" -ne 0 ] || fail "already-running container was not detected"
        grep -q 'already running' <<< "$out" || fail "already-running message is not specific"
        grep -q -- '--shutdown' <<< "$out" || fail "already-running message does not tell the user to pass --shutdown"
        ok "an already-running container is detected and the message names --shutdown"

        out="$(run_sh --config "$GOOD_CONFIG" --status)"
        # podman prints the short (12-character) id; the run above returned the full one.
        grep -q "${CREATED_CONTAINER:0:12}" <<< "$out" || fail "--status did not list the running container"
        ok "--status lists the running container"

        # The owner's real container is on port 8080 with image gent-talk:v0. Neither the
        # throwaway image name nor the throwaway port can match it.
        out="$(run_sh --config "$GOOD_CONFIG" --shutdown)"
        grep -q 'Not rebuilding or relaunching' <<< "$out" || fail "--shutdown did not say it stops and exits"
        sleep 1
        if "$ENGINE" ps --format '{{.ID}}' | grep -q "${CREATED_CONTAINER:0:12}"; then
            fail "--shutdown did not stop the container"
        fi
        ok "--shutdown stops the container and exits without rebuilding"

        out="$(run_sh --config "$GOOD_CONFIG" --shutdown)"
        grep -q 'Nothing to stop' <<< "$out" || fail "--shutdown on nothing running is not a clean no-op"
        ok "--shutdown is idempotent when nothing is running"
    fi
else
    echo "SKIP $ENGINE not available: container-detection checks not run" >&2
fi

# --- 11. no credential ever appears in run.sh's output ----------------------------------------
for secret in "$SENTINEL_BOT_TOKEN" "$SENTINEL_READ_TOKEN" "$SENTINEL_WRITE_TOKEN" "$SENTINEL_EL_KEY"; do
    if grep -qF "$secret" "$ALL_OUTPUT"; then
        fail "a credential value appeared in run.sh output (sentinel ${secret%%-*}...)"
    fi
done
# Positive control: the check above is only meaningful if this file would have caught a leak.
printf '%s\n' "$SENTINEL_READ_TOKEN" > "$TMPDIR_TEST/control.txt"
grep -qF "$SENTINEL_READ_TOKEN" "$TMPDIR_TEST/control.txt" \
    || fail "the leak check itself is broken: it cannot find a value it was just handed"
ok "no credential value appears anywhere in run.sh's output (across $(wc -l < "$ALL_OUTPUT") lines)"

echo ""
echo "all $PASS checks passed"
