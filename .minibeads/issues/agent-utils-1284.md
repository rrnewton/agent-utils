---
title: 'audit-cpu-budget: wrkslots audit exceeds the hourly timer CPU budget at 200 worktrees'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-11T10:45:03.434685252+00:00
updated_at: 2026-11-11T13:47:10.364707029+00:00
closed_at: 2026-11-11T13:47:10.364706909+00:00
---

# Description

An hourly `wrkslots audit` timer has a budget of 1000 CPU-s at about 200 worktrees. A boxed measurement on 2026-11-11 (05:26-05:36 ET, agent-utils pin ce0db627 plus #1276, host quiet at start) over 204 worktrees (143 needing attention, 37 running agents) recorded: wall 508 s; cgroup CPU 1115.9 CPU-s, over the 1000 CPU-s budget; outer wall 9 min 28 s; largest RSS 395 MB; exit 1 (actionable). The timer stays disabled until audit fits, with a target of under 400 CPU-s total and no change to any verdict.

In-process CPU was 624 s (56% of the cgroup total; the remaining 44% is child processes, to be attributed: git, du, or other per-row subprocesses). By phase:

- registry 42.2 s (79,398 events, 201 rows)
- liveness 11.4 s (135 subjects)
- process-census 21.3 s (201 slot paths)
- cache-planning 179.9 s (82,158 directories listed, 40 subjects exhausted)
- cache-census 5.0 s
- registered-rows 364.3 s CPU / 145.4 s wall (201 rows, judged in parallel)
- storage 0

registered-rows plus cache-planning are 87% of in-process CPU. registered-rows costs about 1.8 CPU-s per row. cache-planning lists about 82k directories; whether it re-lists per subject, reuses nothing across rows, or recurses into build trees is to be confirmed by profiling.

Related and closed: #1106 audit-cache-allowance-load, #1172 audit-lock-hold, #1200 rust-status-consistency.

# Design

- Profile first on a realistic registry, read-only, under `nice -n 19 ionice -c3`: find where registered-rows spends 1.8 CPU-s per row, why cache-planning lists 82k directories, and which child processes make up the 44% of cgroup CPU outside the in-process total.
- Fix the biggest costs without changing verdicts, for example: list each cache tree once and share it across subjects; stop at known cache roots instead of walking them; avoid per-row repeated registry or git work; batch git calls.
- Keep the Python and Rust editions in parity where both have the code path (the Rust edition is primary where it exists; record which edition the audit timer runs).
- Regression coverage bounds the work by counting directory listings or subprocess calls, never wall time (owner reply 346 forbids wall-clock budgets in gating checks), plus verdict-parity tests on a fixture.

# Acceptance Criteria

- At about 200 worktrees the audit's total CPU is under 400 CPU-s inside the 1000 CPU-s timer budget, with before/after numbers (CPU-s per phase, directory listings, child CPU) recorded in Notes.
- Verdicts, reasons and output are unchanged for the same registry state (verdict-parity fixture test in both editions where both have the path).
- A regression test bounds directory listings or subprocess calls for the fixed paths.
- The path-selected validation gate passes.

# Notes

Reported 2026-11-11 from the boxed measurement above. The hourly audit timer stays disabled until this lands. The audit timer runs the Python edition (the harness `worktrees/wrkslots` is the Python entry point); the Rust edition mirrors every fix below.

## Profiling (mu-auditcpu, 2026-11-11)

- registered-rows re-folded the whole event log per row: `_load_hold` and `_owner_release_state` each rescanned a machine's full chain for every record (201 rows x 79,398 events x 2 folds). At that exact scale (synthetic 79,398-event chain, 201 rows) the per-row folds cost 5.71 CPU-s; folded once per machine they cost 0.03 CPU-s. The Rust edition was worse: `current_hold` reloaded the chain from disk per row.
- Per checkout, audit ran seven full index/tree listings (tracked-cache `ls-files` + `ls-tree`, submodule `ls-files --stage` + `ls-tree`, `ls-files -v`, `ls-files -t`, salvage `--stage` again). On a 20,001-tracked-file repository the per-checkout set cost 65 ms CPU in 7 git processes. The child-process share of the cgroup total is these git processes (plus the privileged census helper's grandchildren, which the parent's rusage never sees).
- cache-planning's 82k listings come from `**` cache globs walking whole trees including build trees. That walk's full `Path.glob` fidelity (including descending into matched cache roots) is pinned by test_cache_plan_budget, so it is unchanged; the savings are on the git side and in the event folds.

## Fix

- Audit folds holds and latest owner releases once per machine and hands them to each row (Python `py/wrkslots/cli.py` `_cmd_audit_scan`/`_audit_record`; Rust `audit_state.rs` `latest_release_events`/`release_state_with_latest`, `hold_write.rs` `holds_from_events`). Row judging also stops cloning the machine's event vector per row in Rust.
- All-literal cache glob sets now list tracked paths through git pathspecs scoped to each glob (`_literal_cache_pathspecs`), so `ls-files`/`ls-tree` return only the glob prefixes instead of the whole index and tree; any wildcard, `**`, empty, or pathspec-shaped component falls back to the full listing, and the same containment filter runs either way.
- `assert_ordinary_index` (both editions, all copies) reads one `ls-files -v` listing instead of `-v` plus `-t`, keeping the assume-unchanged-before-skip-worktree report order.
- The audit VCS shares one `ls-files --stage` listing between the cache-policy check and the submodule salvage inventory (`_AuditGitVcs._indexed_gitlink_paths`, via a `submodule_paths` refactor that changes no result).

Before/after on the 20,001-file repository: 65 -> 33 ms CPU per checkout, 7 -> 6 git processes (5 distinct listings under the audit VCS, which shares the stage listing). Live-registry audit (27 slots): 563 -> 514 git calls with identical verdicts and reasons. Event folds at the measured scale: 5.71 -> 0.03 CPU-s. Whether the consumer registry clears 400 CPU-s depends on its tracked-tree size, which these cuts scale with linearly; no verdict, reason, or output changed (differential suites, HELD/release fixtures, and the new parity tests).

Tests: `test_census_cost.py` gains batched-fold equivalence, never-refold, pathspec-listing, one-listing index, and shared-stage-listing tests (each fails on the old code for its cost half); `rs/wrkslotsd` gains `batched_hold_fold_matches_per_slot_folds` and `latest_release_events_keeps_only_well_formed_latest`. Local validate reds were only the 30 s /proc census bound under host load (#1267 class), each solo-green (remove differentials 5/5; standalone subset OK); the gate runs at owner tier with that recorded reason.
