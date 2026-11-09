---
title: 'audit-cache-allowance-load: cache-census allowances leave slots UNKNOWN or BLOCKED on a loaded host'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-09T02:10:00.000000000+00:00
updated_at: 2026-11-09T05:00:00.000000000+00:00
closed_at: 2026-11-09T05:00:00.000000000+00:00
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

Decision (the lead, 2026-11-08): keep the verdict DELETABLE when only a cache allowance ran out. JSON: `cache_bytes: null`, plus `size_known: false` and the allowance reason. Human output: `DELETABLE (size unknown: <reason naming the --cache-* knob>)`. Rankers must treat null as unknown, not 0. Document it in the audit output reference. Findings (nested Git, crossings, an unreadable entry, a changed directory) still decide the verdict. Both editions.

## Resolution

Delivered by cl-wrkid, 2026-11-08 ET, in both editions (py/wrkslots/cli.py `_cache_size_unknown_reason`; rs/wrkslotsd/src/audit_state.rs `cache_size_unknown_reason`).

- What counts as an allowance shortfall:
  - a partial census;
  - a cache error whose text ends with the shared `no cache total can be published within this allowance`: subject planning listings, subject planning wall, audit-wide planning total, root binding wall, traversal wall (which includes a nested-Git judgment the wall cut short), finalization work, finalization wall.
  Neither adds a reason any more, so the row keeps the verdict its other evidence gives. Every other cache error still blocks as `cache inspection failed: ...`: nested Git, a symlink crossing, an unreadable entry, a changed directory, a judged nested repository, and unavailable or unpersistable census state.
- Every audit row now carries `size_known` (true exactly when `cache_bytes` is a number) and `size_unknown_reason`. The reason is null unless only an allowance ran out; then it is the allowance message plus the option for a complete total (`--cache-work-limit`, `--cache-wall-seconds` or `--cache-plan-total-seconds`; for a partial census, rerun audit, which resumes from `--cache-census-state`). `cache_error` and `cache_status` keep their values.
- Text output: `DELETABLE (size unknown: <reason>): <slot>: ...`. The `; cache inspection: ...` suffix is printed only for a finding.
- The gate's `unknown_slots` no longer counts a BLOCKED row whose only cache problem is an allowance shortfall. Its other evidence classifies it.
- Rankers: nothing in this repository ranks or sums audit rows by `cache_bytes`. The user guide ("When only an allowance ran out") states that null is unknown, never zero.
- Tests:
  - py/wrkslots/tests/test_audit_state_rust.py, Python/Rust differential:
    - a 30-directory cache under `--cache-work-limit 1` is DELETABLE with the size unknown, then sized on the resumed run;
    - planning listings at 5, the planning total at 1 µs, and the census wall allowance at 1 µs (standing in for a listing slower than the allowance, which is what load does to the 5-second default) are each DELETABLE, with the reason naming the matching option;
    - nested Git in the cache still blocks, with no size reason;
    - the text line is identical in both editions.
  - py/wrkslots/tests/test_lifecycle.py: `test_audit_partial_cache_census_leaves_only_the_size_unknown` (formerly `..._cannot_upgrade_deletion_eligibility`) covers the JSON, the gate (attention, not unknown) and the text line.

