#!/bin/sh
# measure_daemon.sh - Buck2 daemon start/cold/warm cost for one checkout.
#
# Usage:
#   measure_daemon.sh CHECKOUT ISOLATION TARGET OUTDIR
#   measure_daemon.sh CHECKOUT ISOLATION "" OUTDIR VALIDATE_CMD...
#
#   CHECKOUT    Project root of the one checkout under test.
#   ISOLATION   Fresh buck2 isolation dir owned by this run. Only this
#               isolation dir's daemon is ever killed (per-root kill).
#   TARGET      Build target for the CLI workload, or "" to skip it.
#   OUTDIR      Created if needed; receives measurements.csv and
#               build.log. Nothing is read from the caller's checkout.
#   VALIDATE_CMD...  Optional full-validation command (workload B).
#
# Outputs: OUTDIR/measurements.csv with one row per phase
# (workload,repeat,phase,wall_s,rss_kb,pss_kb,memavailable_kb) and
# OUTDIR/build.log with every command's output. Phases per repeat:
# cold (fresh daemon, warm remote cache), warm (no changes).
# Three repeats; callers report medians.
#
# Abort rule: refuses to start, and aborts mid-run, when host
# MemAvailable falls below 100 GB (104857600 kB). Never kills another
# checkout's daemon, never edits any file, never scans an Eden mount.
set -eu
if [ "$#" -lt 4 ]; then
  echo "usage: measure_daemon.sh CHECKOUT ISOLATION TARGET OUTDIR [VALIDATE_CMD...]" >&2
  exit 2
fi
checkout=$1; isolation=$2; target=$3; outdir=$4; shift 4
mkdir -p "$outdir"
csv="$outdir/measurements.csv"
echo "workload,repeat,phase,wall_s,rss_kb,pss_kb,memavailable_kb,kind" > "$csv"
memavail() { awk '/^MemAvailable:/ {print $2}' /proc/meminfo; }
if [ "$(memavail)" -lt 104857600 ]; then
  echo "refusing: MemAvailable below 100 GB" >&2; exit 1
fi
daemon_pids() {
  # Daemon pid for this run, from the daemon's own info file (the
  # daemon cmdline carries neither checkout nor isolation). The info
  # path mirrors the checkout path under the buckd state dir.
  info="$HOME/.buck/buckd/${checkout#/}/$isolation/buckd.info"
  if [ -r "$info" ]; then
    pid=$(sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$info" | head -n 1)
    if [ -n "$pid" ] && [ -d "/proc/$pid" ]; then
      echo "$pid"
      return
    fi
  fi
  # Fallback: cmdline match on extra patterns the caller supplies
  # (for example a logged fresh checkout path).
  for pid in $(pgrep -f "buckd" || true); do
    cmd=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null || true)
    for pattern in ${MEASURE_EXTRA_PATTERNS:-}; do
      case "$cmd" in
        *"$pattern"*) echo "$pid"; break ;;
      esac
    done
  done
}
sample() {
  rss=0; pss=0
  for pid in $(daemon_pids); do
    if [ -r "/proc/$pid/smaps_rollup" ]; then
      r=$(awk '/^Rss:/ {print $2}' "/proc/$pid/smaps_rollup")
      p=$(awk '/^Pss:/ {print $2}' "/proc/$pid/smaps_rollup")
      rss=$((rss + r)); pss=$((pss + p))
    fi
  done
  echo "$rss $pss $(memavail)"
}
run_phase() {
  workload=$1; repeat=$2; phase=$3; shift 3
  start=$(date +%s)
  (cd "$checkout" && "$@") >> "$outdir/build.log" 2>&1 &
  build_pid=$!
  peak_rss=0; peak_pss=0
  while kill -0 "$build_pid" 2>/dev/null; do
    set -- $(sample)
    [ "$1" -gt "$peak_rss" ] && peak_rss=$1
    [ "$2" -gt "$peak_pss" ] && peak_pss=$2
    if [ "$3" -lt 104857600 ]; then
      echo "aborting: MemAvailable below 100 GB" >&2
      kill "$build_pid" 2>/dev/null || true
      exit 1
    fi
    sleep 2
  done
  wait "$build_pid"
  wall=$(( $(date +%s) - start ))
  set -- $(sample)
  echo "$workload,$repeat,$phase,$wall,$peak_rss,$peak_pss,$3,peak" >> "$csv"
  echo "$workload,$repeat,$phase,$wall,$1,$2,$3,idle" >> "$csv"
  if [ "$3" -lt 104857600 ]; then
    echo "aborting: MemAvailable below 100 GB" >&2; exit 1
  fi
}
for repeat in 1 2 3; do
  (cd "$checkout" && buck2 --isolation-dir "$isolation" kill) \
    >> "$outdir/build.log" 2>&1 || true
  if [ -n "$target" ]; then
    run_phase cli "$repeat" cold \
      buck2 --isolation-dir "$isolation" build "$target"
    run_phase cli "$repeat" warm \
      buck2 --isolation-dir "$isolation" build "$target"
  fi
  if [ "$#" -gt 0 ]; then
    run_phase validate "$repeat" cold "$@"
    run_phase validate "$repeat" warm "$@"
  fi
done
echo "done: $csv"
