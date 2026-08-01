#!/usr/bin/env bash
# box-run.sh — run ONE command inside safe-ci-dag-runner's cgroup box: a hard per-run
# memory cap (inner cgroup memory.max), a hard wall-clock timeout, and a SETSID-PROOF
# whole-subtree teardown (cgroup.kill), so a wedging/leaking command can neither OOM the
# host nor leave escaped qemu/supervisor processes alive after it is killed.
#
# This is the single-command front end to `safe-ci-dag-runner run` (which is a DAG runner):
# it emits a one-step DAG, runs it boxed with --no-fail-fast, and classifies the outcome.
#
# Usage:
#   box-run.sh --mem 6G --timeout 175 [--label NAME] [--perf-dir DIR]
#              [--verdicts-csv FILE] [--allow-unboxed] [-q] -- CMD [ARGS...]
#
#   --mem SIZE          hard per-run memory cap (K/M/G/T suffix or raw bytes). Required.
#   --timeout SECS      hard wall-clock cap; the whole subtree is SIGKILLed at SECS. Required.
#   --label NAME        label for the verdict line / CSV row (default: "run").
#   --perf-dir DIR      write safe-ci-dag-runner per-step + whole-run resource CSVs here.
#   --verdicts-csv FILE append one row: label,class,exit,wall_s,detail  (header auto-created).
#   --allow-unboxed     if cgroup boxing cannot be established, DEGRADE to process-group
#                       teardown instead of failing. NOT recommended for demo5 wedges: a
#                       plain pgroup kill misses setsid/double-fork escapees.
#   -q                  quiet: only the final VERDICT line on stdout.
#
# Exit status: 0 iff the command PASSED (exit 0 within cap). Otherwise non-zero, and the
# CLASS (TIMEOUT / OOM / FAIL / BOX-UNAVAILABLE) is printed on the VERDICT line and CSV.
# The wrapped command's own file outputs (e.g. a qemu `-serial file:` console log) are
# produced normally, so a downstream classifier can still read them.
#
# The safe-ci-dag-runner binary is resolved from, in order:
#   $SAFE_CI_DAG_RUNNER_BIN, ../rs/target/release/, ../rs/target/debug/ (relative to this
#   script), then $PATH.
set -euo pipefail

die() { echo "box-run.sh: $*" >&2; exit 2; }

mem=""; timeout=""; label="run"; perf_dir=""; verdicts_csv=""; allow_unboxed=0; quiet=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mem)          mem="${2:?}"; shift 2 ;;
    --timeout)      timeout="${2:?}"; shift 2 ;;
    --label)        label="${2:?}"; shift 2 ;;
    --perf-dir)     perf_dir="${2:?}"; shift 2 ;;
    --verdicts-csv) verdicts_csv="${2:?}"; shift 2 ;;
    --allow-unboxed) allow_unboxed=1; shift ;;
    -q|--quiet)     quiet=1; shift ;;
    --)             shift; break ;;
    -h|--help)      sed -n '2,40p' "$0"; exit 0 ;;
    *)              die "unknown option '$1' (did you forget '--' before the command?)" ;;
  esac
done
[[ -n "$mem" ]]     || die "--mem is required"
[[ -n "$timeout" ]] || die "--timeout is required"
[[ $# -gt 0 ]]      || die "no command given (put it after '--')"

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

# --- parse --mem to bytes ------------------------------------------------------------------
to_bytes() {
  local s="${1//[[:space:]]/}"; local n unit
  if [[ "$s" =~ ^([0-9]+)([KkMmGgTt]?[Bb]?)$ ]]; then
    n="${BASH_REMATCH[1]}"; unit="${BASH_REMATCH[2]}"
    case "${unit^^}" in
      ""|"B")      echo "$n" ;;
      "K"|"KB")    echo $(( n * 1024 )) ;;
      "M"|"MB")    echo $(( n * 1024 * 1024 )) ;;
      "G"|"GB")    echo $(( n * 1024 * 1024 * 1024 )) ;;
      "T"|"TB")    echo $(( n * 1024 * 1024 * 1024 * 1024 )) ;;
      *)           return 1 ;;
    esac
  else
    return 1
  fi
}
mem_bytes="$(to_bytes "$mem")" || die "cannot parse --mem '$mem' (use e.g. 6G, 512M, 2048K, or raw bytes)"

# --- build the one-step DAG (command joined into one properly-quoted bash string) ----------
cmd_str="$(printf '%q ' "$@")"; cmd_str="${cmd_str% }"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
dag="$work/dag.json"
python3 - "$dag" "$label" "$cmd_str" "$timeout" "$mem_bytes" <<'PY'
import json, sys
dag, label, cmd, timeout, mem = sys.argv[1:6]
json.dump({"steps": [{
    "group": "box", "job": label, "cmd": cmd,
    "timeout": int(timeout),
    "hint": {"hard_mem_max_bytes": int(mem)},
}]}, open(dag, "w"))
PY

# --- run boxed -----------------------------------------------------------------------------
run_args=(run --dag "$dag" -j1 --no-fail-fast)
[[ -n "$perf_dir" ]] && run_args+=(--perf-dir "$perf_dir")
[[ "$allow_unboxed" -eq 1 ]] && run_args+=(--allow-cgroup-failure)

out="$work/out.txt"
start=$SECONDS
set +e
"$bin" "${run_args[@]}" >"$out" 2>&1
rc=$?
set -e
wall=$(( SECONDS - start ))

[[ "$quiet" -eq 0 ]] && { grep -vE 'profile data appended|^  \.safe-ci-dag-runner|\.csv$' "$out" >&2 || true; }

# --- classify from the outcome + exit code -------------------------------------------------
# rc: 0 all-pass | 1 a step failed | 3 cgroup boxing required but unavailable | 2 bad usage
detail="$(grep -oE '(OOM-KILLED \(hit inner MemoryMax; [0-9]+ oom_kill event\(s\)\)|TIMEOUT >[0-9]+s|exit -?[0-9]+)' "$out" | tail -1 || true)"
if [[ "$rc" -eq 3 ]]; then
  class="BOX-UNAVAILABLE"; ec=3
elif grep -q 'OOM-KILLED' "$out"; then
  class="OOM"; ec=1
elif grep -q 'TIMEOUT >' "$out"; then
  class="TIMEOUT"; ec=1
elif [[ "$rc" -eq 0 ]] && grep -q '\bPASS\b' "$out"; then
  class="PASS"; ec=0
else
  class="FAIL"; ec="${rc:-1}"; [[ "$ec" -eq 0 ]] && ec=1
fi

echo "VERDICT label=$label class=$class exit=$ec wall_s=$wall detail=${detail:-none}"
if [[ -n "$verdicts_csv" ]]; then
  [[ -f "$verdicts_csv" ]] || echo "label,class,exit,wall_s,detail" >"$verdicts_csv"
  printf '%s,%s,%s,%s,%s\n' "$label" "$class" "$ec" "$wall" "${detail:-none}" >>"$verdicts_csv"
fi
exit "$ec"
