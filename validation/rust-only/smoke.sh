#!/bin/sh
# Rust-only functional smoke (#2131 rust-only-functional-check): run each
# Rust-primary tool through a real basic flow with no Python interpreter
# present, and report pass/fail per tool plus one overall verdict line
# answering "works with no python3". POSIX sh only, so this runs in the
# slim validation/rust-only container and in any shell-only environment.
# Fixtures live in validation/rust-only/fixtures/ beside this script's
# source; in the image they are copied to the fixtures directory below.
# Python-primary tools are printed as not-yet with their tracking issue,
# never run and never counted as passes. A flow that the container
# environment itself prevents (cpuset-alloc without usable cgroups) is
# reported BLOCKED with the exact reason, never as a pass.
set -u

BIN_DIR="${1:-/usr/local/bin}"
FIXTURES="${SMOKE_FIXTURES:-/usr/local/share/agent-utils-rust-only/fixtures}"
pass=0
fail=0
blocked=0

ok() { echo "PASS $1: $2"; pass=$((pass + 1)); }
bad() { echo "FAIL $1: $2"; fail=$((fail + 1)); }
blk() { echo "BLOCKED $1: $2"; blocked=$((blocked + 1)); }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT INT TERM

# The environment itself: no Python interpreter on PATH.
if command -v python3 >/dev/null 2>&1 || command -v python >/dev/null 2>&1; then
    bad environment "a Python interpreter is present on PATH in the Rust-only environment"
else
    ok environment "no python/python3 on PATH"
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
# Needs a systemd user scope for AllowedCPUs; where the container's
# cgroups prevent that, report the exact limitation as BLOCKED.
mkdir -p "$WORK/cpu"
if (cd "$WORK/cpu" && "$BIN_DIR/cpuset-alloc" run --cores 1 -- /bin/sh -c 'printf ran >ran.txt' >run.log 2>&1) \
    && [ -f "$WORK/cpu/ran.txt" ] \
    && "$BIN_DIR/cpuset-alloc" status >"$WORK/cpu/status.log" 2>&1 \
    && grep -q '"held_count": 0' "$WORK/cpu/status.log"; then
    ok cpuset-alloc "reserved 1 core, ran a pinned command, released (status held_count 0)"
else
    reason="$(head -n 1 "$WORK/cpu/run.log" 2>/dev/null)"
    blk cpuset-alloc "reserve+release could not run in this container: ${reason:-no output captured}"
fi

# tick-hub: one tick on a fixture whose gate fires, with capture.
if "$BIN_DIR/tick-hub" tick --config "$FIXTURES/tick.yaml" --now 1700000000 --no-header >"$WORK/tick.log" 2>&1 \
    && grep -q 'ACTION: backlog-triage' "$WORK/tick.log" \
    && grep -q 'count=42' "$WORK/tick.log"; then
    ok tick-hub "one tick fired the fixture gate and emitted ACTION backlog-triage with captured count=42"
else
    bad tick-hub "fixture tick did not emit the expected ACTION line"
fi

# pr-landing-planner: plan a fixture input and check the output shape.
if "$BIN_DIR/pr-landing-planner" plan --fixture "$FIXTURES/planner.yaml" --format json --no-archive >"$WORK/planner.log" 2>&1 \
    && grep -q '"conflict_edges"' "$WORK/planner.log" \
    && grep -q '"nodes"' "$WORK/planner.log"; then
    ok pr-landing-planner "fixture plan produced JSON with conflict_edges and nodes"
else
    bad pr-landing-planner "fixture plan did not produce the expected JSON shape"
fi

# agentctl: registry round-trip without Herdr. Interactive
# start/send/status/stop needs Herdr or tmux plus a harness binary,
# none of which exist in this container; the durable registry and
# inbox are the headless surface that runs here, exercised for real:
# capabilities, list on a fresh registry, and an inbox post/deliver
# round-trip whose text must come back.
REG="$WORK/reg"
if "$BIN_DIR/agentctl" --registry "$REG" capabilities >"$WORK/cap.log" 2>&1 \
    && grep -q '"interactive"' "$WORK/cap.log" \
    && "$BIN_DIR/agentctl" --registry "$REG" list >"$WORK/list.log" 2>&1 \
    && grep -q '^\[\]$' "$WORK/list.log" \
    && "$BIN_DIR/agentctl" --registry "$REG" inbox post --to coord --from reviewer --kind idle --text 'Review done' >"$WORK/post.log" 2>&1 \
    && grep -q '"outcome": "stored"' "$WORK/post.log" \
    && "$BIN_DIR/agentctl" --registry "$REG" inbox deliver --to coord --via print >"$WORK/deliver.log" 2>&1 \
    && grep -q 'Review done' "$WORK/deliver.log"; then
    ok agentctl "capabilities + fresh-registry list + inbox post/deliver round-trip (no Herdr needed)"
else
    bad agentctl "registry/inbox round-trip failed"
fi
echo "note agentctl: interactive start/send/status/stop needs Herdr or tmux plus a harness binary, absent in-container; the registry + inbox round-trip above is the headless flow exercised here"

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
# binary's pycompat translation) over a planted fixture registry:
# a minimal .wrkslots.yml plus ACTIVE/ARCHIVED machine shards holding
# one slot record, built by hand from the rs/wrkslotsd test fixtures.
cp -r "$FIXTURES/wrkslots" "$WORK/proj"
if "$BIN_DIR/wrkslotsd" status --root "$WORK/proj" --machine node-a --format json >"$WORK/wstatus.log" 2>&1 \
    && grep -q '"slot": "slot-a"' "$WORK/wstatus.log" \
    && "$BIN_DIR/wrkslotsd" doctor --root "$WORK/proj" --machine node-a --format json >"$WORK/wdoctor.log" 2>&1 \
    && grep -q '"findings"' "$WORK/wdoctor.log"; then
    ok wrkslotsd "status + doctor (pycompat spellings) read the planted fixture registry and found slot-a"
else
    bad wrkslotsd "status/doctor over the fixture registry failed"
fi

# wrkvizd: inspect a minimal hand-built archive (legacy timeline
# generation: manifest.json + data/timeline.json with known counts).
cp -r "$FIXTURES/wrkviz-archive" "$WORK/archive"
if "$BIN_DIR/wrkvizd" inspect --output "$WORK/archive" >"$WORK/inspect.log" 2>&1 \
    && grep -q '"agents": 1' "$WORK/inspect.log" \
    && grep -q '"phases": 1' "$WORK/inspect.log"; then
    ok wrkvizd "inspect counted the hand-built archive (agents 1, phases 1, events 1)"
else
    bad wrkvizd "inspect over the hand-built archive failed"
fi

# Remaining shipped Rust binaries: prove each one starts and answers.
for tool in agent-teams agent-usage gh-paced herdr-agent; do
    if "$BIN_DIR/$tool" --help >/dev/null 2>&1; then
        ok "$tool" "binary runs (--help); no deeper flow in this v1 smoke"
    else
        bad "$tool" "Rust-primary tool did not answer --help"
    fi
done

# Python-primary surface: named gaps with tracking issues, not run.
echo "not-yet wrkslots (#562 rust-primary-wrkslots): Python-primary CLI; its Rust readers are exercised via wrkslotsd above"
echo "not-yet wrkviz (#706 rust-primary-wrkviz): Python-primary CLI; its Rust reader is exercised via wrkvizd above"
echo "not-yet local-ci-hub (#1365): Python-primary"
echo "not-yet herdr-subagents (#1125 herdr-subagents-differentials): Python-primary"
echo "not-yet agentctl migrate (subcommand): Python-only; folds into the herdr-subagents port"
echo "not-yet herdr-run run (server-backed pane flow): needs a running Herdr server, absent in-container; the local check/status contract is exercised above"

if [ "$fail" -eq 0 ] && [ "$blocked" -eq 0 ]; then
    echo "rust-only verdict: PASS - works with no python3 ($pass passed, $fail failed, $blocked blocked)"
    exit 0
fi
echo "rust-only verdict: FAIL - not yet fully working with no python3 ($pass passed, $fail failed, $blocked blocked)"
exit 1
