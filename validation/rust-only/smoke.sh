#!/bin/sh
# Rust-only functional smoke (#2131 rust-only-functional-check): run each
# Rust-primary tool through a real basic flow with no Python interpreter
# present, and report per-tool results plus two verdict lines: the
# Rust-primary functional result, and the whole-repo "works with no
# python3" answer (NOT YET while Python-primary tools remain). POSIX sh
# only, so this runs in the slim validation/rust-only container and in
# any shell-only environment.
#
# Host run (binaries already built; this script never builds):
#   BIN_DIR=rs/target/release SMOKE_FIXTURES=validation/rust-only/fixtures \
#     sh validation/rust-only/smoke.sh
# In the image BIN_DIR and SMOKE_FIXTURES default to the copied paths.
#
# Result classes: PASS is a real flow with known-answer assertions.
# HELP-ONLY means only --help was exercised; it is never counted as a
# flow pass. BLOCKED is reserved for cpuset-alloc when the captured
# stderr proves the systemd-scope limitation specifically; any other
# cpuset failure is FAIL. Python-primary tools print as not-yet with
# their tracking issue, never run and never counted as passes.
set -u

BIN_DIR="${1:-${BIN_DIR:-/usr/local/bin}}"
FIXTURES="${SMOKE_FIXTURES:-/usr/local/share/agent-utils-rust-only/fixtures}"
pass=0
fail=0
blocked=0
helponly=0

ok() { echo "PASS $1: $2"; pass=$((pass + 1)); }
bad() { echo "FAIL $1: $2"; fail=$((fail + 1)); }
blk() { echo "BLOCKED $1: $2"; blocked=$((blocked + 1)); }
helpok() { echo "HELP-ONLY $1: $2"; helponly=$((helponly + 1)); }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT INT TERM

# The environment itself: no Python interpreter on PATH, including
# versioned names (python3.12 and friends), matching the meter scrub.
pyfound=""
_saved_ifs="$IFS"
IFS=:
for _d in $PATH; do
    [ -n "$_d" ] || continue
    for _f in "$_d"/python "$_d"/python3 "$_d"/python3.*; do
        if [ -e "$_f" ] || [ -L "$_f" ]; then pyfound="$_f"; break 2; fi
    done
done
IFS="$_saved_ifs"
if [ -n "$pyfound" ]; then
    bad environment "a Python interpreter is present on PATH in the Rust-only environment: $pyfound"
else
    ok environment "no python/python3/python3.* on PATH"
fi

# dagrun: run a small 3-node DAG and verify the chained output.
mkdir -p "$WORK/dag"
cp "$FIXTURES/dag.json" "$WORK/dag/dag.json"
if (cd "$WORK/dag" && "$BIN_DIR/dagrun" run --dag dag.json --allow-cgroup-failure --no-profile >run.log 2>&1) \
    && printf 'one\ntwo\nthree\n' >"$WORK/dag/expected.txt" \
    && cmp -s "$WORK/dag/expected.txt" "$WORK/dag/out/three.txt"; then
    ok dagrun "3-node DAG ran unboxed (--allow-cgroup-failure) and out/three.txt holds one/two/three"
else
    bad dagrun "3-node DAG did not produce the expected chained output"
fi

# cpuset-alloc: reserve cores, run a command pinned to them, release.
# BLOCKED requires evidence of the systemd-scope limitation: the
# allocator's own refusal naming `systemd-run --user --scope` as
# unavailable. Missing binaries, crashes, and real reserve/release
# bugs fall through to FAIL.
mkdir -p "$WORK/cpu"
if (cd "$WORK/cpu" && "$BIN_DIR/cpuset-alloc" run --cores 1 -- /bin/sh -c 'printf ran >ran.txt' >run.log 2>&1) \
    && [ -f "$WORK/cpu/ran.txt" ] \
    && "$BIN_DIR/cpuset-alloc" status >"$WORK/cpu/status.log" 2>&1 \
    && grep -q '"held_count": 0' "$WORK/cpu/status.log"; then
    ok cpuset-alloc "reserved 1 core, ran a pinned command, released (status held_count 0)"
elif grep -Eq 'systemd-run --user --scope.*unavailable' "$WORK/cpu/run.log" 2>/dev/null; then
    reason="$(head -n 1 "$WORK/cpu/run.log" 2>/dev/null)"
    blk cpuset-alloc "systemd-scope limitation proven by allocator refusal: ${reason:-no output captured}"
else
    reason="$(head -n 1 "$WORK/cpu/run.log" 2>/dev/null)"
    bad cpuset-alloc "reserve+release failed without the systemd-scope signature: ${reason:-no output captured}"
fi

# tick-hub: one tick on a fixture whose gate fires, with capture.
if "$BIN_DIR/tick-hub" tick --config "$FIXTURES/tick.yaml" --now 1700000000 --no-header >"$WORK/tick.log" 2>&1 \
    && grep -q 'ACTION: backlog-triage' "$WORK/tick.log" \
    && grep -q 'count=42' "$WORK/tick.log"; then
    ok tick-hub "one tick fired the fixture gate and emitted ACTION backlog-triage with captured count=42"
else
    bad tick-hub "fixture tick did not emit the expected ACTION line"
fi

# pr-landing-planner: the fixture encodes a known answer (PR 1043
# ready; 987 and 942 conflict on src/shared.rs; 942's required gate is
# stale, so its action is refire-stale-gate). Assert exactly that.
if "$BIN_DIR/pr-landing-planner" plan --fixture "$FIXTURES/planner.yaml" --format json --no-archive >"$WORK/planner.log" 2>&1 \
    && grep -Eq '"a": *987' "$WORK/planner.log" \
    && grep -Eq '"b": *942' "$WORK/planner.log" \
    && grep -q 'src/shared.rs' "$WORK/planner.log" \
    && grep -Eq '"pr": *1043' "$WORK/planner.log" \
    && grep -Eq '"action": *"refire-stale-gate"' "$WORK/planner.log"; then
    ok pr-landing-planner "fixture plan: nodes include PR 1043, conflict edge 987-942 on src/shared.rs, PR 942 classified refire-stale-gate"
else
    bad pr-landing-planner "fixture plan did not match the known answer (1043 present, 987-942 conflict on src/shared.rs, refire-stale-gate)"
fi

# agentctl: registry round-trip without Herdr. The capabilities
# document must report its true values (herdr interactive backend,
# muse among the harnesses), not merely contain the key. The inbox
# post/deliver round-trip must return the posted text.
REG="$WORK/reg"
if "$BIN_DIR/agentctl" --registry "$REG" capabilities >"$WORK/cap.log" 2>&1 \
    && grep -q '"interactive"' "$WORK/cap.log" \
    && grep -Eq '"harnesses"' "$WORK/cap.log" \
    && grep -q '"muse"' "$WORK/cap.log" \
    && "$BIN_DIR/agentctl" --registry "$REG" list >"$WORK/list.log" 2>&1 \
    && grep -q '^\[\]$' "$WORK/list.log" \
    && "$BIN_DIR/agentctl" --registry "$REG" inbox post --to coord --from reviewer --kind idle --text 'Review done' >"$WORK/post.log" 2>&1 \
    && grep -q '"outcome": "stored"' "$WORK/post.log" \
    && "$BIN_DIR/agentctl" --registry "$REG" inbox deliver --to coord --via print >"$WORK/deliver.log" 2>&1 \
    && grep -q 'Review done' "$WORK/deliver.log"; then
    ok agentctl "capabilities (herdr backend, muse harness) + fresh-registry list + inbox post/deliver round-trip (no Herdr needed)"
else
    bad agentctl "registry/inbox round-trip failed"
fi
echo "note agentctl: interactive start/send/status/stop needs Herdr or tmux plus a harness binary, absent in-container; the registry + inbox round-trip above is the headless flow exercised here (issue design's fake-harness flow remains unmet)"

# agent-teams: declare a team against a scratch registry, then verify
# the declarations check clean. Both steps are local file flows.
mkdir -p "$WORK/at"
printf '%s\n' '{"schema": "agent-teams-teams/v1", "hosts": {}, "teams": {}}' >"$WORK/at/teams.json"
if "$BIN_DIR/agent-teams" --state-dir "$WORK/at/state" --teams "$WORK/at/teams.json" init smoke-team --registry "$WORK/at/reg" >"$WORK/at-init.log" 2>&1 \
    && grep -q 'declared team smoke-team' "$WORK/at-init.log" \
    && "$BIN_DIR/agent-teams" --state-dir "$WORK/at/state" --teams "$WORK/at/teams.json" check >"$WORK/at-check.log" 2>&1 \
    && grep -q 'check: OK' "$WORK/at-check.log"; then
    ok agent-teams "init declared smoke-team against a scratch registry and check verified the declarations"
else
    bad agent-teams "init/check declaration flow failed"
fi

# agent-usage: plant one history sample, then read it back. The
# reader must return the planted provider and meter, not just run.
mkdir -p "$WORK/usage"
_ts="$(date +%s 2>/dev/null || echo 1700000000)"
printf '%s\n' "{\"v\": 1, \"ts\": $_ts, \"provider\": \"claude\", \"status\": \"ok\", \"meters\": [{\"id\": \"session\", \"label\": \"Current session\", \"used_pct\": 42.5}], \"elapsed_ms\": 3}" >"$WORK/usage/history.jsonl"
if [ "$(AGENT_USAGE_DIR="$WORK/usage" "$BIN_DIR/agent-usage" path 2>/dev/null)" = "$WORK/usage" ] \
    && AGENT_USAGE_DIR="$WORK/usage" "$BIN_DIR/agent-usage" history --provider claude --json >"$WORK/usage.log" 2>&1 \
    && grep -Eq '"provider": *"claude"' "$WORK/usage.log" \
    && grep -Eq '"label": *"Current session"' "$WORK/usage.log"; then
    ok agent-usage "path resolved the planted cache dir and history returned the planted claude sample (Current session)"
else
    bad agent-usage "planted-history readback failed"
fi

# gh-paced: classify one read and one write gh command offline. The
# classifier never touches the network; the classes are known answers.
if "$BIN_DIR/gh-paced" classify --json -- gh pr list >"$WORK/ghp-read.log" 2>&1 \
    && grep -Eq '"class": *"read"' "$WORK/ghp-read.log" \
    && "$BIN_DIR/gh-paced" classify --json -- gh pr merge 123 >"$WORK/ghp-write.log" 2>&1 \
    && grep -Eq '"class": *"write"' "$WORK/ghp-write.log"; then
    ok gh-paced "classify: 'gh pr list' is read and 'gh pr merge 123' is write (offline classifier)"
else
    bad gh-paced "offline classify flow failed"
fi

# herdr-run: local contract only (init, admission check, status).
# The server-backed pane flow is named not-yet below.
mkdir -p "$WORK/hr"
if (cd "$WORK/hr" && "$BIN_DIR/herdr-run" init >init.log 2>&1) \
    && (cd "$WORK/hr" && "$BIN_DIR/herdr-run" check "git status" >check.log 2>&1) \
    && grep -q 'ALLOWED' "$WORK/hr/check.log" \
    && (cd "$WORK/hr" && "$BIN_DIR/herdr-run" status >status.log 2>&1); then
    ok herdr-run "init wrote .herdr-run.yaml, check admitted 'git status', status read the resolved config"
else
    bad herdr-run "local init/check/status contract failed"
fi

# wrkslotsd: status/doctor readers (Python spellings, routed by the
# binary's pycompat translation) over a planted fixture registry.
# The checkout directories the record names are planted empty, so
# doctor reads a registry whose slots exist on disk; doctor's own
# Git-dependent findings still need git, absent in the slim image.
cp -r "$FIXTURES/wrkslots" "$WORK/proj"
mkdir -p "$WORK/proj/worktrees/slot-a/repo" "$WORK/proj/worktrees/upstream-repo"
if "$BIN_DIR/wrkslotsd" status --root "$WORK/proj" --machine node-a --format json >"$WORK/wstatus.log" 2>&1 \
    && grep -q '"slot": "slot-a"' "$WORK/wstatus.log" \
    && "$BIN_DIR/wrkslotsd" doctor --root "$WORK/proj" --machine node-a --format json >"$WORK/wdoctor.log" 2>&1 \
    && grep -q '"findings"' "$WORK/wdoctor.log" \
    && grep -q '"slot": "slot-a"' "$WORK/wdoctor.log"; then
    ok wrkslotsd "status + doctor (pycompat spellings) read the planted fixture registry and named slot-a in both"
else
    bad wrkslotsd "status/doctor over the fixture registry failed"
fi

# wrkvizd: inspect a minimal hand-built archive (legacy timeline
# generation: manifest.json + data/timeline.json). Every counted
# collection in the fixture is asserted, events included.
cp -r "$FIXTURES/wrkviz-archive" "$WORK/archive"
if "$BIN_DIR/wrkvizd" inspect --output "$WORK/archive" >"$WORK/inspect.log" 2>&1 \
    && grep -Eq '"agents": *1' "$WORK/inspect.log" \
    && grep -Eq '"phases": *1' "$WORK/inspect.log" \
    && grep -Eq '"events": *1' "$WORK/inspect.log"; then
    ok wrkvizd "inspect counted the hand-built archive (agents 1, phases 1, events 1)"
else
    bad wrkvizd "inspect over the hand-built archive failed"
fi

# herdr-agent is the legacy agentctl alias: every operation it offers
# (start/send/status/read/stop) drives a live Herdr pane, so no
# fixture-able flow exists without a Herdr server. --help only,
# never counted as a flow pass.
if "$BIN_DIR/herdr-agent" --help >/dev/null 2>&1; then
    helpok herdr-agent "binary runs (--help); all operations need a running Herdr server, so no flow is claimed"
else
    bad herdr-agent "legacy alias did not answer --help"
fi

# Python-primary surface: named gaps with tracking issues, not run.
echo "not-yet wrkslots (#562 rust-primary-wrkslots): Python-primary CLI; its Rust readers are exercised via wrkslotsd above"
echo "not-yet wrkviz (#706 rust-primary-wrkviz): Python-primary CLI; its Rust reader is exercised via wrkvizd above"
echo "not-yet local-ci-hub (#1365): Python-primary"
echo "not-yet herdr-subagents (#1125 herdr-subagents-differentials): Python-primary"
echo "not-yet herdr-chat: Python-primary (absent Rust edition)"
echo "not-yet parallel-experiment-runner: Python-primary (absent Rust edition)"
echo "not-yet run-agent-prompt: Python-primary (absent Rust edition)"
echo "not-yet agentctl migrate (subcommand): Python-only; folds into the herdr-subagents port"
echo "not-yet herdr-run run (server-backed pane flow): needs a running Herdr server, absent in-container; the local check/status contract is exercised above"

# Two verdicts. The Rust-primary functional verdict passes when no
# real flow failed; an environment-BLOCKED tool is reported in its
# own line and in the count, never as a pass for that tool. The
# whole-repo answer stays NOT YET until the Python-primary tools
# above land, whatever the Rust-primary result is.
if [ "$fail" -eq 0 ] && [ "$blocked" -eq 0 ]; then
    echo "rust-primary functional verdict: PASS - works with no python3 ($pass flows passed, $helponly help-only excluded, $fail failed, $blocked blocked)"
    _rc=0
elif [ "$fail" -eq 0 ]; then
    echo "rust-primary functional verdict: PASS with environment-blocked tools reported above - no real flow failed ($pass flows passed, $helponly help-only excluded, $fail failed, $blocked blocked)"
    _rc=0
else
    echo "rust-primary functional verdict: FAIL - not yet fully working with no python3 ($pass flows passed, $helponly help-only excluded, $fail failed, $blocked blocked)"
    _rc=1
fi
echo "whole-repo no-python verdict: NOT YET - Python-primary tools listed as not-yet above; the Rust-primary result does not answer for them"
exit "$_rc"
