#!/usr/bin/env bash
# box-run.sh — run ONE command inside safe-ci-dag-runner's cgroup box: a hard per-run
# memory cap (inner cgroup memory.max), an optional per-run CPU cap (inner cgroup cpu.max),
# a hard wall-clock timeout, and a SETSID-PROOF whole-subtree teardown (cgroup.kill), so a
# wedging/leaking command can neither OOM the host nor leave escaped qemu/supervisor
# processes alive after it is killed.
#
# This is a thin front end to the runner's native one-off boxer, `safe-ci-dag-runner box`
# (which synthesizes a singleton in-memory DAG, runs it once, and prints a VERDICT). This
# wrapper adds binary resolution, an --allow-unboxed alias, and optional CSV logging.
#
# Usage:
#   box-run.sh --mem 6G --timeout 175 [--cores N] [--label NAME] [--perf-dir DIR]
#              [--verdicts-csv FILE] [--allow-unboxed] [-q] -- CMD [ARGS...]
#
#   --mem SIZE          hard per-run memory cap (K/M/G/T suffix or raw bytes). Required.
#   --timeout SECS      hard wall-clock cap; the whole subtree is SIGKILLed at SECS. Required.
#   --cores N           cap the boxed command to N full CPUs (inner cgroup cpu.max). Optional.
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

# --- delegate to the native `box` subcommand -----------------------------------------------
box_args=(box --mem "$mem" --timeout "$timeout" --label "$label")
[[ -n "$cores" ]]              && box_args+=(--cores "$cores")
[[ -n "$perf_dir" ]]          && box_args+=(--perf-dir "$perf_dir")
[[ "$allow_unboxed" -eq 1 ]]  && box_args+=(--allow-cgroup-failure)
[[ "$quiet" -eq 1 ]]          && box_args+=(-q)

work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
out="$work/out.txt"
set +e
"$bin" "${box_args[@]}" -- "$@" 2>&1 | tee "$out"
rc="${PIPESTATUS[0]}"
set -e

# --- parse the VERDICT line the native subcommand printed ----------------------------------
verdict_line="$(grep -E '^VERDICT ' "$out" | tail -1 || true)"
class="$(sed -n 's/.*class=\([^ ]*\).*/\1/p' <<<"$verdict_line")"
wall="$(sed -n 's/.*wall_s=\([^ ]*\).*/\1/p' <<<"$verdict_line")"
detail="$(sed -n 's/.*detail=\(.*\)$/\1/p' <<<"$verdict_line")"
: "${class:=FAIL}" "${wall:=0.0}" "${detail:=none}"

if [[ -n "$verdicts_csv" ]]; then
  [[ -f "$verdicts_csv" ]] || echo "label,class,exit,wall_s,detail" >"$verdicts_csv"
  printf '%s,%s,%s,%s,%s\n' "$label" "$class" "$rc" "$wall" "$detail" >>"$verdicts_csv"
fi
exit "$rc"
