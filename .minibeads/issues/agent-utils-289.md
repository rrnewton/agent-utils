---
title: 'census-zombie-leader-threads: treat a zombie thread-group leader with live threads as alive in the slot-use census'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-10-14T14:48:59.062857319+00:00
updated_at: 2026-10-14T18:09:30.707990307+00:00
closed_at: 2026-10-14T18:09:30.707990157+00:00
---

# Description

[opus 5.5] Found by the 2026-10-14 branch triage (ai_docs/transient/2026-10-14-branch-triage/REPORT.md). On main, _process_is_zombie and _process_generation_is_terminal (py/wrkslots/cli.py) treat any leader in State Z as terminal, and the census callers (_absent_validate_process_snapshot, _preflight_process_observations, _same_uid_process_observations, _process_observation_is_active, _agent_batch_fenced_use, and the lsof fallback) skip it without looking further. A process whose main thread called pthread_exit shows 'State: Z' with 'Threads: 2': the leader's cwd reads ENOENT, while a surviving thread, which /proc readdir does not list, still holds its cwd and open descriptors in the target. Reproduced on a host on 2026-10-14. The census then reports that directory unused, which is a fail-open. Main already knows the hazard: 2bd7f956 requires 'Threads: 1' for the exiting-namespace-init exemption. Prior work: commit abfac4c6 on archived/salvage/agent-utils-wrkslots-final-review-20261010 (_zombie_leader_thread_dirs, _zombie_leader_has_live_threads); leave out its mount-view keying, which c66e9e19 rejected as unsound. The same branch holds two lower-value pieces: one named nested-Git policy for validate slots so that audit predicts what removal deletes (the audit walk refuses every nested .git while _remove_cache_directory allows them for completed validation), and audit --cache-census-only (62ed952e) with d8fcbcf3's churn handling, worth porting only after re-measuring main's current census at host scale.

# Acceptance Criteria

A zombie leader with live threads is refused or observed through /proc/PID/task/TID in every census path; a real pthread_exit test whose surviving thread holds a descriptor in the target proves the slot is reported in use; make validate passes.
