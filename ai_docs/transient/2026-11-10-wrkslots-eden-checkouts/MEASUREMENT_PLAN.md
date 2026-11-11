# Buck2 daemon cost measurement plan (owner reply 301)

Nothing here has run. Execution waits for the coordinator to relay
which checkout and time window to use. All numbers below are
expectations for sizing, not results.

## Goal

Quantify daemon startup and warm-up cost on two real workloads so
the teardown policy (idle timeout T) can be set from evidence:
kill after T idle when holding the daemon's memory longer costs
more than the cold restart it saves.

## Workloads

- A: one modest CLI build target in the assigned checkout (named
  in the run record before the first build; a few minutes cold).
- B: the buck-based full validation entry point in the same
  checkout (named before the first run).

## Method (3 repeats each, report medians)

Per workload and repeat: kill only that checkout's daemon
(per-root kill, never a machine-wide kill), record daemon start
wall time, run the build cold (fresh daemon, warm remote cache),
run it again warm with no changes, apply one small edit, run
again. Sample daemon RSS/PSS from the daemon's smaps_rollup at
idle after each phase and track the peak during builds. A watcher
samples MemAvailable every 5 s and aborts the whole run below
100 GB. Durations: expect about 1.5 h for A (3x cold+warm+edit)
and about 3 h for B; runs happen only in the assigned window.

## Script

`measure_daemon.sh CHECKOUT TARGET OUTDIR [VALIDATE_CMD...]`
writes one CSV row per phase (workload, repeat, phase, wall_s,
rss_kb, pss_kb, memavailable_kb) and refuses to start when
MemAvailable is already below the abort line. It never kills
another checkout's daemon and never edits outside the one small
probe edit, which it reverts.

## Teardown rule (to fill from medians)

Hold cost per idle hour is the daemon's idle PSS held; restart
cost is cold-minus-warm wall time valued as blocked build time.
Recommend T where the two cross, stated with both medians, in the
issue design and the harness report with a CHANGELOG bullet.
