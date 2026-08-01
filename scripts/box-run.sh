#!/usr/bin/env bash
# box-run.sh — run ONE command inside safe-ci-dag-runner's cgroup box.
#
# One-liner boxed run: take a command + resource flags, EMIT a temporary singleton-DAG JSON
# describing a single boxed step, RUN it via `safe-ci-dag-runner run` (which applies a hard
# per-run memory cap via inner cgroup memory.max, an optional per-run CPU cap via inner cgroup
# cpu.max, a hard wall-clock timeout, and a SETSID-PROOF whole-subtree teardown via cgroup.kill),
# CAPTURE the command's output, then DELETE the temp JSON. A wedging/leaking command can neither
# OOM the host nor leave escaped qemu/supervisor processes alive after it is killed.
#
# This wrapper depends only on the runner's STABLE `run` subcommand and its DAG-JSON schema —
# not on any newer subcommand — so it works with any runner build that supports cgroup boxing.
#
# Usage:
#   box-run.sh --mem 6G --timeout 175 [--cores N] [--label NAME] [--perf-dir DIR]
#              [--verdicts-csv FILE] [--allow-unboxed] [-q] -- CMD [ARGS...]
#
#   --mem SIZE          hard per-run memory cap (K/M/G/T suffix, optional 'i'/'B', or raw bytes).
#                       Required.
#   --timeout SECS      hard wall-clock cap; the whole subtree is SIGKILLed at SECS. Required.
#   --cores N           cap the boxed command to N full CPUs (inner cgroup cpu.max). Optional.
#   --label NAME        label for the step / verdict line / CSV row (default: "run").
#   --perf-dir DIR      write safe-ci-dag-runner per-step + whole-run resource CSVs here.
#   --verdicts-csv FILE append one row: label,class,exit,wall_s,detail  (header auto-created).
#   --allow-unboxed     if cgroup boxing cannot be established, DEGRADE to process-group teardown
#                       instead of failing. NOT recommended for wedge repro: a plain pgroup kill
#                       misses setsid/double-fork escapees.
#   -q                  quiet: suppress the runner's per-step chatter; still prints the VERDICT.
#
# Exit status mirrors the runner: 0 the command PASSED; 1 it FAILED (nonzero/OOM/timeout);
# 2 bad usage; 3 boxing required but unavailable. The CLASS (PASS/TIMEOUT/OOM/FAIL/BOX-UNAVAILABLE)
# is printed on the VERDICT line and CSV. The wrapped command's own file outputs (e.g. a qemu
# `-serial file:` console log) are produced normally, so a downstream classifier can read them.
#
# The safe-ci-dag-runner binary is resolved from, in order:
#   $SAFE_CI_DAG_RUNNER_BIN, ../rs/target/release/, ../rs/target/debug/ (relative to this
#   script), then $PATH.
set -euo pipefail

die() { echo "box-run.sh: $*" >&2; exit 2; }

mem=""; timeout=""; cores=""; label="run"; perf_dir=""; verdicts_csv=""; allow_unboxed=0; quiet=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mem)          mem="${2:?}"; shift 2 ;;
    --timeout)      timeout="${2:?}"; shift 2 ;;
    --cores)        cores="${2:?}"; shift 2 ;;
    --label)        label="${2:?}"; shift 2 ;;
    --perf-dir)     perf_dir="${2:?}"; shift 2 ;;
    --verdicts-csv) verdicts_csv="${2:?}"; shift 2 ;;
    --allow-unboxed) allow_unboxed=1; shift ;;
    -q|--quiet)     quiet=1; shift ;;
    --)             shift; break ;;
    -h|--help)      sed -n '2,36p' "$0"; exit 0 ;;
    *)              die "unknown option '$1' (did you forget '--' before the command?)" ;;
  esac
done
[[ -n "$mem" ]]     || die "--mem is required"
[[ -n "$timeout" ]] || die "--timeout is required"
[[ "$timeout" =~ ^[0-9]+$ ]] || die "--timeout must be a whole number of seconds (got '$timeout')"
[[ -z "$cores" || "$cores" =~ ^[0-9]+$ && "$cores" -ge 1 ]] || die "--cores must be an integer >= 1 (got '$cores')"
[[ $# -gt 0 ]]      || die "no command given (put it after '--')"
cmd=( "$@" )

# --- resolve the runner binary -------------------------------------------------------------
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin=""
for cand in \
    "${SAFE_CI_DAG_RUNNER_BIN:-}" \
    "$here/../rs/target/release/safe-ci-dag-runner" \
    "$here/../rs/target/debug/safe-ci-dag-runner" ; do
  [[ -n "$cand" && -x "$cand" ]] && { bin="$cand"; break; }
done
[[ -z "$bin" ]] && bin="$(command -v safe-ci-dag-runner || true)"
[[ -n "$bin" ]] || die "safe-ci-dag-runner binary not found (set \$SAFE_CI_DAG_RUNNER_BIN or 'cargo build --release' in rs/)"

# --- EMIT the temporary singleton-DAG JSON -------------------------------------------------
# python3 both parses the --mem size (matching the runner's 1024-based K/M/G/T convention) and
# safely JSON-encodes the command (shell-quoted + joined, since the runner runs it via `bash -c`).
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
dag_json="$work/singleton-dag.json"

MEM="$mem" TIMEOUT="$timeout" CORES="$cores" LABEL="$label" \
python3 - "$dag_json" "${cmd[@]}" <<'PY'
import json, os, re, shlex, sys

def parse_size(s):
    s = s.strip()
    m = re.fullmatch(r'(\d+(?:\.\d+)?)\s*([kKmMgGtT]?)[iI]?[bB]?', s)
    if not m:
        sys.exit(f"box-run.sh: could not parse --mem size '{s}'")
    val, suf = float(m.group(1)), m.group(2).lower()
    mult = {'': 1, 'k': 1024, 'm': 1024**2, 'g': 1024**3, 't': 1024**4}[suf]
    return int(val * mult)

out_path = sys.argv[1]
cmd = sys.argv[2:]                      # the user's command + args
cmd_str = shlex.join(cmd)              # -> a single string the runner feeds to `bash -c`

hint = {"classification": "cpu-bound",
        "hard_mem_max_bytes": parse_size(os.environ["MEM"])}
cores = os.environ.get("CORES", "")
if cores:
    hint["preferred_inner_jobs"] = int(cores)

step = {
    "group": "box",
    "job": os.environ["LABEL"],
    "desc": "one-off boxed run",
    "cmd": cmd_str,
    "timeout": int(os.environ["TIMEOUT"]),
    # empty jobs_flag => the runner never appends `-jN` to the user's command; the command owns
    # its own concurrency, while `preferred_inner_jobs` still drives the cgroup cpu.max cap.
    "jobs_flag": "",
    "hint": hint,
}
with open(out_path, "w") as f:
    json.dump({"steps": [step]}, f, indent=2)
PY

# --- RUN the singleton DAG via the stable `run` subcommand (boxing is ON by default) --------
run_args=(run --dag "$dag_json")
[[ -n "$perf_dir" ]]          && run_args+=(--perf-dir "$perf_dir")
[[ "$allow_unboxed" -eq 1 ]]  && run_args+=(--allow-cgroup-failure)
[[ "$quiet" -eq 1 ]]          && run_args+=(-q)

out="$work/out.txt"
start=$SECONDS
set +e
"$bin" "${run_args[@]}" 2>&1 | tee "$out"
rc="${PIPESTATUS[0]}"
set -e
wall=$(( SECONDS - start ))

# --- CLASSIFY from the runner's exit code + failure-reason strings --------------------------
# The runner prints canonical reasons: "OOM-KILLED (hit inner MemoryMax; ...)" and "TIMEOUT >Ns".
detail="$(grep -oE 'OOM-KILLED \(hit inner MemoryMax; [0-9]+ oom_kill event\(s\)\)|TIMEOUT >[0-9]+s|exit [0-9]+|signal [A-Z0-9]+' "$out" | tail -1 || true)"
if   grep -q 'OOM-KILLED'  "$out"; then class="OOM"
elif grep -q 'TIMEOUT >'   "$out"; then class="TIMEOUT"
elif [[ "$rc" -eq 3 ]];            then class="BOX-UNAVAILABLE"
elif [[ "$rc" -eq 0 ]];            then class="PASS"
else                                    class="FAIL"
fi
: "${detail:=none}"

echo "VERDICT label=${label} class=${class} exit=${rc} wall_s=${wall} detail=${detail}"

if [[ -n "$verdicts_csv" ]]; then
  [[ -f "$verdicts_csv" ]] || echo "label,class,exit,wall_s,detail" >"$verdicts_csv"
  printf '%s,%s,%s,%s,%s\n' "$label" "$class" "$rc" "$wall" "$detail" >>"$verdicts_csv"
fi

# temp JSON (and the whole $work dir) removed by the EXIT trap
exit "$rc"
