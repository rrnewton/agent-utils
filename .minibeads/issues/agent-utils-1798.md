---
title: 'wrkslots-ignored-changed-refusal-names-paths: name each changed path and why in the salvage recheck refusal'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-18T11:00:00-05:00
updated_at: 2026-11-18T11:00:00-05:00
---

# Description

[watermelon-full-preview] A consumer `remove` ran for 58 minutes and then refused at the end with "checkout X gitignored files changed after salvage; preserve it and rerun remove", naming no paths. No file outside `.git` had a newer mtime, so the change was likely in which files count as ignored: a `.gitignore` or exclude change in a shared parent during the run changed the membership, not any file's content. The operator cannot tell what to restore or rerun from the message.

# Design

- The salvage recheck refusal (and the finish-recorded recheck with the same shape) names each changed path and says why it counts as changed: new, removed, content changed (size or sha256), or ignored-status changed, and by which ignore rule where that is knowable.
- Consider judging against the salvage-time ignore set rather than re-evaluating the rules at recheck time, so a rule change in a shared parent is reported as what it is instead of as file changes.
- Both editions (Python `py/wrkslots/cli.py` and Rust `rs/wrkslotsd/src/remove_write.rs`), with differential parity.

# Acceptance Criteria

- A recheck that refuses names every differing path with its reason; a membership change caused by an ignore-rule change is reported as an ignored-status change, naming the rule.
- Tests cover both cases: a content change and an ignore-membership change between salvage and recheck.
- Existing remove and finish differential tests stay green.

# Notes

Assigned to the mu-evidencecap lane after #1772 wrkslots-ignored-evidence-unbounded-events landed (e6ee5a82). The recheck code is `_assert_receipt_ignored_files` in `py/wrkslots/cli.py` and its Rust counterpart in `rs/wrkslotsd/src/remove_write.rs`; with #1772 the expected set may come from the manifest receipt, so the diff is computed against the manifest entries.
