---
title: 'gate-records-head-at-start-refuses-midrun-change: a gate row whose HEAD or working tree changed during the run is recorded invalid, never pass, and land-main never accepts it'
status: open
priority: 1
issue_type: task
assignee: cl-flaketriage
labels:
- gate
- ledger
created_at: 2026-11-26T18:08:00-05:00
updated_at: 2026-11-26T18:08:00-05:00
---

# Description

`scripts/gate.py` writes the ledger row at the end of a run, and `_record_ledger` reads `head_sha` from `git rev-parse HEAD` at that moment. A lane that commits, amends, rebases or edits files in the gated worktree while its gate runs gets a row that names the NEW head with the verdict of the OLD tree. The landing tool matches gate evidence by the tree of `head_sha`, so such a row can vouch for a commit no gate ever ran. Nothing records the head or tree the run actually started on, so the row cannot be told apart from an honest one afterwards.

# Design

- At admission (just before the admitted command starts), record `HEAD` and the working-tree hash (`_working_tree_hash`: staged, unstaged and untracked, the same hash validate.py uses).
- At the end, read both again. When either differs, write the row with verdict `invalid` (never `pass` or `partial`, whatever the command's exit), `head_sha` set to the head the run started on, and new fields naming both: `head_sha_at_start`, `head_sha_at_end`, `tree_at_start`, `tree_at_end`, plus a one-line reason; print the same reason at the end of the gate output so the lane sees it.
- Unchanged runs record `head_sha_at_start` and `tree_at_start` too, so a reader can confirm the row's tree.
- `gate.py stats` counts invalid rows apart from passes and fails.
- The landing tool refuses a row whose verdict is `invalid` (and never treats a later rerun as rescuing it), naming both SHAs; a fresh gate on the final tree is the only way forward.

# Acceptance Criteria

- A gate whose `HEAD` changes during the run records verdict `invalid` with both SHAs, `head_sha` equal to the start head, even when the command exits 0; the gate output says so.
- A gate whose working tree changes during the run (an uncommitted edit or a new untracked file) also records `invalid` with both tree hashes.
- An unchanged run records the same verdict as before plus `head_sha_at_start` / `tree_at_start`.
- The landing tool refuses an `invalid` row, including when an exclusive rerun row on the same tree passed afterwards.
- Tests cover all of the above with a fake command that commits or edits mid-run; no wall-clock budget (owner rule 346).

# Notes

Brief `cl-flaketriage` (coordb, 2026-11-26), after `#2308`. Keep the `gate.py` hunk in the admission/ledger code only (`#2207` and `#2316` also touch `gate.py`). `base_sha` is likewise read at the end; the start value is recorded alongside so a moved remote-tracking ref is visible too. The landing tool lives in the harness repository, so its half lands there under that repository's own rules.
