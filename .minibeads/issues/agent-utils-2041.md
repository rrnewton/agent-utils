---
title: 'wrkslots-deferred-finish-journal-race: a sidecarred finish journal refuses its own out-of-lock reload'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T07:00:00-05:00
updated_at: 2026-11-21T08:28:43-05:00
closed_at: 2026-11-21T08:28:43-05:00
---

# Description

[watermelon-full-preview] Field reports (a remove at a build containing #1735, and a closing `finish` on another host): an out-of-lock removal refuses rc 3 with "deferred finish journal changed before its out-of-lock deletion" (and the sibling "private finish journal changed" shapes), leaving the slot fenced with its row present, in RECOVERY REQUIRED. No concurrent writer is needed.

Cause: `_finish_journal_payload` sidecars `containing_remote_refs` in the *in-memory* journal (#1757): any checkout or salvage holder over the 64-ref inline limit gains a `containing_remote_refs_sidecar` key. `_write_journal` writes that form; a journal loaded back has the sidecars expanded and the keys deleted. The four out-of-lock identity compares (`_load_prepared_private_finish`, `_complete_prepared_private_finish`, `_delete_prepared_deferred_finish`, `_complete_deferred_finish`) compared those two forms with strict whole-JSON equality, so any removal whose salvage inventory exceeds 64 refs — routine under `--archive-private-history` with hundreds of rescue tips — refused its own journal deterministically.

# Design

- Compare finish journals in canonical expanded form at the four identity-compare sites: deep-copy both sides, expand sidecars, then the ordinary whole-JSON comparison. Every other check is unchanged, so a substituted or tampered journal still refuses.
- No lock change, no journal format or location change, no persisted-state change. The compares are Python-only; the Rust edition has no out-of-lock deferred/private finish compare (its finishes stay locked).

# Acceptance Criteria

- Deterministic regression tests, one per out-of-lock shape: a plain deferred remove and the private finish reload, each with a salvage inventory over the inline limit, complete instead of refusing.
- A negative control: a journal whose content was substituted after the prepare still refuses at the same compare.
- The existing fenced lock-scope and nested-repair suites still pass.

# Notes

Related: #1757 (finish-journal sidecars), #1735 wrkslots-remove-holds-state-lock-through-salvage (the out-of-lock compares), #2026 wrkslots-fence-breaks-nested-submodule-gitdir (the stale-`core.worktree` sibling).

## Resolution

[watermelon-full-preview] Landed as d236117e on github/main. The four out-of-lock identity compares now canonicalise both journals (expand #1757 sidecars on copies) before the whole-JSON comparison, so a sidecarred finish journal reloads as itself; every other check is unchanged, a substituted journal still refuses, and a missing sidecar file fails closed. Regression tests: a deferred remove and the private reload, each with a salvage inventory over the 64-ref inline limit and failing first with the exact field refusals, plus the substituted-journal negative control and the missing-sidecar fail-closed test. Gate on 51e6f1c5 was red only on the #1957 census-churn test (passes alone on the tree and on main; solo rerun green).
