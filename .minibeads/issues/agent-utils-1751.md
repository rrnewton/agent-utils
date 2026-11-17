---
title: 'wrkslots-agent-batch-census-reruns-every-item: remove-agent-batch retakes the shared lsof census for every slot instead of reusing it within its 300 s age bound'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T22:28:20.000000000+00:00
updated_at: 2026-11-16T22:28:20.000000000+00:00
---

# Description

Owner-ordered host-wide slot cleanup: `wrkslots remove-agent-batch` takes
about 95-100 s per agent slot before the registry lock (46 slots is about
77 minutes). A py-spy profile from the field (1 Hz, items 5-6 of a batch
of 8, 195 samples) attributes about 25 s (26%) of an item to the shared
process census (`_capture_agent_remove_batch_census` plus
`_run_bounded_scan`, lsof-based).

That census re-runs on every item, although
`_AGENT_REMOVE_BATCH_CENSUS_MAX_AGE_SECONDS = 300` and the
`remove-agent-batch` help text promise that one lsof scan and one reading
of /proc cover every requested slot and are repeated only when older than
300 seconds. In `_remove_agent_batch`, the per-item check
`slot_path not in context.censused` can never pass for a later item:
each capture covers only the current item's slot (#1367), so
`context.censused` holds only that one slot and the next item's path
always misses, forcing a fresh census per item. The age check against
`context.captured_at` therefore never decides anything.

# Design

- Capture the shared census over every remaining eligible slot, not only
  the current item's slot, and retake it only when the current item's
  slot is not covered or the census is older than
  `_AGENT_REMOVE_BATCH_CENSUS_MAX_AGE_SECONDS`, exactly as the help text
  promises.
- Keep every per-item check the help text names: at each process-use
  check before the fence, confirm the slot directory is the one the scan
  covered, refuse on any use that scan saw or any lsof warning that may
  concern the slot, and look up use in the shared /proc reading; the
  decisive per-item scan after the path fence is unchanged.
- Out of scope: the git salvage / private-history subprocess cost in the
  same profile (about 28 s) is a separate lane's fix (#1437); do not
  touch `_submodule_private_history`, `_pregather_salvage_facts`,
  `_salvage_candidate_uncached`, `_reflog_reaches`, or the Rust
  `submodule_private_history`.

# Acceptance Criteria

- Deterministic regression test with a stubbed census counter: N items
  of one batch within the census age bound take exactly one census
  (fails before the fix, passes after).
- A census older than the age bound is still retaken, and an item whose
  slot directory is no longer the one the census covered still refuses
  per item.
- `remove-agent-batch` does not exist in the Rust edition (it is in the
  `PY_ONLY` list of the Rust CLI), so the fix and test are Python-only.
- Checks wait on conditions, not wall-clock budgets.

# Notes

[watermelon-full-preview] Field profile split for one batch item: shared
census about 25 s (26%, this issue), git salvage / private-history
subprocesses about 28 s (#1437, another lane), the referenced-ignored
paths warning about 20 s (21%, #1752).

## Resolution

[watermelon-full-preview] Working as designed; the defect was the
documentation, not the code. The per-slot census is the deliberate
#1367 design (issue agent-utils-1367, Design (3) and its counted
acceptance: "One lsof//proc census per slot per removal"), landed as
191aa5cc with counted tests: each item captures only its own slot at
its turn so a process that enters a later slot after the batch
starts is seen by that slot's own pre-fence scan, and a slow batch
never rescans every remaining slot tree. The `remove-agent-batch`
help text, the USER_GUIDE batch section and the
`_AGENT_REMOVE_BATCH_CENSUS_MAX_AGE_SECONDS` comment still described
the earlier batch-wide shared scan (#1175) and were corrected to
the per-slot behaviour in the landing commit; code and tests are
unchanged. The census cost itself stands as designed; the batch
speed blockers were #1753 (ignored-files refusal after salvage)
and #1752 (warning scan per slot).
