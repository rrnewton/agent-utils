---
title: 'wrkslots-warn-ignored-paths-referenced-per-slot-cost: compute the referenced-ignored-paths scan once per batch instead of once per slot'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T22:28:20.000000000+00:00
updated_at: 2026-11-16T22:28:20.000000000+00:00
---

# Description

Owner-ordered host-wide slot cleanup: `wrkslots remove-agent-batch` takes
about 95-100 s per agent slot before the registry lock. A py-spy profile
from the field (1 Hz, items 5-6 of a batch of 8, 195 samples) attributes
about 20 s (21%) of an item to `_warn_ignored_paths_referenced`
(`py/wrkslots/cli.py`).

That helper is warning-only (#1302): for each slot with gitignored files
it recursively scans the source tree's `.minibeads/issues`, `ai_docs` and
`reviews` records (bounded at 1000 files and 32 MiB), reads them, and
substring-searches every ignored path, printing the same warnings again
for every slot of a batch. The scanned tree is the same for every item,
so the whole scan repeats per slot for identical output.

# Design

- Compute the referenced-ignored-paths scan once per batch: cache the
  scanned corpus keyed by the repository tree it reads (the project
  root and a fingerprint of the scanned files), so a later slot in the
  same batch reuses it instead of re-reading the tree.
- Keep the warning output identical: the same warnings, in the same
  order, for the same inputs; the bounded scan limits (1000 files,
  32 MiB) and the warning-only behaviour are unchanged.
- Cheaper search (for example `git grep -F`) is welcome only if it
  preserves that output exactly, including files git does not track;
  otherwise the cached Python scan stands.

# Acceptance Criteria

- Deterministic regression test: two slots with gitignored files in one
  batch produce one referenced-paths scan and the identical warnings
  (fails before the fix, passes after).
- Warning output for a single removal is unchanged.
- `remove-agent-batch` does not exist in the Rust edition (it is in the
  `PY_ONLY` list of the Rust CLI), and the helper's Python behaviour is
  the contract, so the fix and test are Python-only.
- Checks wait on conditions, not wall-clock budgets.

# Notes

[watermelon-full-preview] Field profile split for one batch item: the
referenced-ignored-paths warning about 20 s (21%, this issue), the
shared census about 25 s (26%, #1751), git salvage / private-history
subprocesses about 28 s (#1437, another lane).
