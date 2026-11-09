---
title: 'audit-cache-allowance-load: cache-census allowances leave slots UNKNOWN or BLOCKED on a loaded host'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T02:10:00.000000000+00:00
updated_at: 2026-11-09T02:39:22.000000000+00:00
assignee: cl-wrkid
---

# Description

On a consumer host with 78 slots at load 100-200, `wrkslots audit` (Python edition, agent-utils a0eeb67d) reports many slots UNKNOWN or BLOCKED only because cache inspection ran out of its allowance:

- `cache inspection failed: cache planning exhausted this subject's share of 3297 directory listings; no cache total can be published within this allowance` (the number varies per subject: 1373, 1636, ...)
- `cache inspection: cache census traversal exhausted the fixed wall allowance of 5 seconds; no cache total can be published within this allowance`
- `cache inspection incomplete: bounded census is partial`

Two runs minutes apart gave different verdicts (DELETABLE 0, then 7), so these allowances make audit's answer depend on host load. The process/path census part of the same evidence is #1102 wrkslots-census-churn-tolerant; this issue covers only the cache-census allowances.

# Design

- Decide what a cache-census shortfall should mean for the row. A missing cache TOTAL is a size-reporting gap, not evidence that the slot is in use. Should it ever block DELETABLE, or only leave the size unknown?
- If the allowances stay, make them deterministic under load (a listing budget rather than a wall clock), or let the operator raise them for one run, and say in the reason how to.
- Python and Rust editions alike, or record the parity gap.

# Acceptance Criteria

- A test with a large cache tree under a forced-small allowance shows the chosen row verdict, the same in both editions.
- The 5-second wall allowance no longer decides a verdict under load (test with an injected slow listing).
- The reason text tells the operator how to get a complete answer.

# Notes

Reported by the lead (devbig014) on 2026-11-08 from a consumer's audit output. Related: #1102 wrkslots-census-churn-tolerant, #1107 audit-format-label-mismatch.

What the audit's cache inspection protects (cl-wrkid, 2026-11-08, written before any verdict change, as the lead asked). Read on main at 31a8198a, py/wrkslots/cli.py.

- In audit it does two things. (1) Byte accounting: the `cache_bytes` total. (2) An early warning of what a deletion would refuse: the bounded walk refuses on nested `.git` inside an agent row's cache (`cache directory contains nested Git metadata`, in the resumable census loop), on a cache path that crosses a symlink, on an entry it cannot inspect, and on a directory that changed between resumed runs; validation rows' nested repositories are judged once the root is verified and final (`_judge_audited_nested_repositories`). Its own comment says why a partial census keeps a row out of DELETABLE: a walk that has not reached every entry may not yet have met a path the complete traversal would refuse, so its byte count "is not deletion evidence".
- No removal proof depends on it. Every path that deletes a cache re-enforces the same protections at deletion time, independent of audit: `_remove_cache_directory` -> `_allocated_open_directory` refuses any `.git` not already judged disposable (`_assert_judged_git_metadata`), a crossing onto another filesystem, and a crossing of a mount point. `wrkslots remove` (including the private, deferred and recovered finishes, all through `_finish_remove_paths`) first judges every nested repository a deletion of each cache would reach, under the path fence (`_judge_cache_nested_repositories` -> `_judge_nested_git_repository`): it keeps one in a verified bundle or refuses. Retirement and `clean-caches` delete through the same `_remove_cache_directory` guard.
- So it is accounting plus a prediction of a refusal that the deleting command makes itself. A missing total means the size is unknown AND audit cannot yet say whether remove will refuse on a nested repository; it is not evidence the slot is in use, and it is not a removal safeguard. (Audit's DELETABLE is already advisory, #1102.)

Consequence for the design (proposal, for the lead): a cache-census shortfall must stop deciding the row's verdict only where the shortfall is about the walk's allowance (listings or wall time), never where the walk FOUND something (nested Git, a symlink crossing, an unreadable entry, a changed directory): those stay blocking reasons. An allowance shortfall would publish the row with the size unknown and a reason that says remove still judges nested repositories itself, and how to get a complete total (the --cache-work-limit / --cache-wall-seconds flags, or a resumed run with --cache-census-state). Whether such a row is DELETABLE or gets a distinct label is the lead's call; both editions alike.

