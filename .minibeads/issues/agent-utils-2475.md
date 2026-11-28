---
title: 'gate-cpu-growth-2475: CPU per passing gate rose after 2026-11-26 for herdr-run.differential (p90 620 to 1135 CPU-s) and packages.standalone.wrkslots (p90 193 to 261); find what grew'
status: open
priority: 1
issue_type: task
labels:
- validation
- hot-host
- herdr-run
- wrkslots
created_at: 2026-11-28T12:05:00-05:00
updated_at: 2026-11-28T12:05:00-05:00
---

# Description

Two CPU-budgeted gate steps reached their budgets in the week to 2026-11-28 without any change of their own.

- **`cross.herdr-run.differential`** was red on 13 gate heads in 7 days, 12 of which never touched herdr-run. Each red was `CPU-TIMEOUT >1200s cpu` at 972 to 1017 s wall.
- **`packages.standalone.wrkslots`** was red on 28 heads, with `CPU-TIMEOUT >300s cpu` (317 s; one gate measured 301 s against 300).

The table below sums user and system CPU from the per-step profile that every gate prints, over passing runs only. The cut is the morning of 2026-11-26, about 6:30 AM ET.

| Step | Budget before this change | Passing runs before the cut: n, p50, p90, max (CPU-s) | After the cut: n, p50, p90, max |
|---|---|---|---|
| `cross.herdr-run.differential` | 1200 | 14, 584, 620, 867 | 28, 772, 1135, 1168 |
| `packages.standalone.wrkslots` | 300 | 32, 145, 193, 227 | 46, 215, 261, 288 |
| `rust.merge-queue.test` (#2458) | 2400 | 13, 1704, 1830, 1865 | 20, 2098, 2365, 2394 |

- herdr-run grew by 515 CPU-s at p90 (+83%) and 188 CPU-s at p50 (+32%).
- standalone wrkslots grew by 68 CPU-s at p90 (+35%) and 70 CPU-s at p50 (+48%).
- merge-queue (#2458) shows the same shape: +29% at p90.
- At the old budgets, the slowest passing runs used 97% and 96% of them.

This change raises the two hang guards to about twice the new hot-host p90: herdr-run 1200 to 2400 CPU-s in `validation/cross.dag.yaml`, standalone wrkslots 300 to 600 CPU-s in `validation/packages.dag.yaml`. That stops the reds. It does not explain the growth, which is what this issue is for.

# Design

The work is to establish, per step, whether the code under test does more work, or the same work costs more CPU on a busier host.

1. Read per-case timings (the case-timings adapter) for `cross/differential.py --tool herdr-run` and for `scripts/check_standalone.py --subset wrkslots`, from gates before and after the cut, and name the cases that grew.
2. List what changed between 2026-11-25 and 2026-11-27 in their inputs and in the shared test wrappers both run under:
   - `cross/`, `py/herdr_run`, `rs/herdr-run` and the wrkslots package;
   - the private temp root (#2383), the PID-namespace and host-shard moves (#2065), and the git stand-in.
3. Measure instructions as well as CPU-seconds for the grown cases, solo and at load (load1 at or above 237). The same instruction count with more CPU-seconds means load inflation; more instructions means more work. The owner's 2026-11-21 rule asks cost checks to use load-invariant measures.
4. Fix what grew if it is waste: a busy wait, a retry loop, or a census that scales with the host. Otherwise record the new basis.

# Acceptance Criteria

- Each of the two steps has a named cause for the growth, with the measurement that shows it (per-case CPU and, where it matters, instructions).
- The budgets end at no less than twice the hot-host p90 of passing runs after any fix, with that basis written beside each budget.
- If a real cost check is wanted, it counts instructions, not CPU-seconds or wall time.

# Notes

Filed by `cl-redaudit` (number pre-assigned by the coordinator, 2026-11-28), from the frequent-red gate audit of 2026-11-28.

- Owner rule 346 applies: the budgets are hang guards, never speed budgets.
- `packages.standalone.wrkslots` also declares `est_duration_s: 45`, against about 280 to 410 s wall in recent passing gates. The scheduler hint is stale, and correcting it is in scope here.

Related:
- #2458: the same growth shape in `rust.merge-queue.test`.
- #2411: page-slice CPU headroom.
- #2219: product CPU budgets, a different thing.
- #2383 and #2065: wrappers that changed in the window.
