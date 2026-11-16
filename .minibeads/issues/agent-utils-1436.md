---
title: 'wrkslots-create-journal-hook-drift: classify, complete and abort a create journal against its own recorded hooks'
status: closed
priority: 0
issue_type: bug
assignee: mu-hookdrift
labels:
- wrkslots
created_at: 2026-11-16T19:00:00+00:00
updated_at: 2026-11-16T19:00:00+00:00
---

# Description

A create was in flight for one slot when the configuration's post-provision hooks for its slot type changed. Every wrkslots create on the host then refused with "host fault: journal ... blocks create-journal classification: create journal post-provision hooks differ from configuration", and `wrkslots recover` (retry and abort modes) refused on the same check, while quarantine-journal was ineligible (owner alive, under 24 h): a deadlock with no supported way out, unblocked by hand by restoring the old hook list in the configuration.

Cause: all three journal readers compare the journal's recorded `post_provision_hooks` with the current configuration for that slot type: `_completed_create_record`, `_create_journal_items` and `_recover_create` in `py/wrkslots/cli.py`, and the matching readers in `rs/wrkslotsd/src/journal_state.rs` and `rs/wrkslotsd/src/classify_journals.rs`. Each raises "create journal post-provision hooks differ from configuration". The host-wide blast radius comes from `_classify_create_journals`, which turns any one journal's refusal into a host fault for every unrelated create.

A second refusal from the same incident: with the hook check bypassed, `recover --abort-create` refused "cannot abort changed checkout ...: source state is dirty (... 160000 ... <submodule>)". Five submodule clones were never checked out (no index; HEAD at the gitlink commit; the directory holds only the `.git` pointer), so the superproject's status reads them as staged deletions. #1431 forgives that partial-init shape only when the journal holds the submodule-init hook at `failed` or `failed-to-start`; the live journal held the hook at `completed`, so abort still refused.

# Design

- A create journal is classified, completed and aborted against its own recorded `post_provision_hooks`, since it describes what was actually started. A difference from the current configuration becomes an audit note (in the journal handling output and in doctor), never a refusal. Every other journal integrity check (planned/created prefixes, progress bounds, failure policy) stays exactly as strict.
- Scope: one slot's stranded or unclassifiable create journal refuses only operations on that slot, never every unrelated create on the host. When a caller names the slot it operates on, classification skips a journal that provably concerns a different slot (from its scoped filename or its parseable bytes); the stranded slot's own operations, and any journal that proves no slot, keep the host-fault refusal with its remedy. Successfully classified journals keep every existing cross-check (duplicate slot or agent claims, path and branch overlap).
- Python and Rust parity: the same change in the Rust classification and completed-record readers. Tests for hook drift mid-create on both the abort path and the complete/resume path, in both editions where the path exists.
- `wrkslots doctor` reports a create journal whose hooks differ from the current configuration (informational, with the slot name).
- Abort: a submodule that was uninitialized at create start, has no index, and whose HEAD equals the recorded gitlink counts as unchanged whatever the journal's hook status (not started, running, completed or failed). The forgiveness stays narrow: never forgive a submodule with a checked-out tree (any files beyond the `.git` pointer, or an index) or with local commits (HEAD off the recorded gitlink, or any local branch or reflog entry not on a remote-tracking ref). Every other #1337 condition is kept.

# Acceptance Criteria

- A create interrupted mid-flight, followed by a configuration hook change, aborts cleanly with `recover --abort-create` and completes with `recover`, running the journal's recorded hooks (not the current configuration's).
- Classification of a drifted journal succeeds and carries the drift as an audit note; doctor reports the drift informationally with the slot name.
- A second create for a different slot succeeds while another slot's journal is stranded or unclassifiable, and operations on the stranded slot still refuse with the remedy.
- The never-checked-out submodule shape aborts cleanly at every hook status (completed first, as in the live incident); a checked-out tree, an index, a moved HEAD, local commits, or authored files still refuse and preserve everything.
- Rust classification matches the Python edition byte-for-byte on a drifted journal; any edition lacking a path says so in the Resolution.

# Notes

Live incident (2026-11-16, another team's slot on this host): one slot's create was in flight when that team changed one post-provision hook in their wrkslots configuration; every create on the host refused on the hook comparison and recover deadlocked on the same check. They unblocked by hand, temporarily restoring the old hook list in their configuration (the hook-revert window), and cleared the abort refusal by resetting the five never-checked-out submodules. Post-hoc evidence from the incident: the module gitdirs were slot-private (`.git/worktrees/<slot>/modules/<name>`), `core.worktree` pointed into the slot, there was no `extensions.worktreeConfig`, and the primary submodules were unchanged after the resets. This fix makes the hand unblock unnecessary.

Overlap at assignment: two lanes on another host touch wrkslots create/journal code (an Eden mount-root create change and a doctor timing change followed by a post-provision hook change). Rebase onto whatever lands first.

## Resolution

- Python (`py/wrkslots/cli.py`): the three readers no longer refuse on hook drift. `_create_journal_items` and `_completed_create_record` validate progress against the journal's recorded hooks; `_recover_create` resumes and aborts against them and `_run_post_provision_hooks` now runs the hooks the journal records. Drift surfaces as an audit note on the classification row (`hook_drift`, plus an `AUDIT:` line in human output), as an `audit note:` line in recover output, and as a `create-journal-hook-drift` doctor finding naming the slot.
- Scope: `_classify_create_journals` skips a journal that provably concerns a different slot than the caller's named slot when that journal fails classification; same-slot operations and slotless journals keep the host-fault refusal with its remedy.
- Abort: `_abort_dirt_forgiven` also forgives dirt that is only never-checked-out submodule clones (only the `.git` pointer in the directory, no index, HEAD at the recorded gitlink, no local commits on HEAD, any local branch, or any reflog entry), at any hook status; all other #1337/#1431 conditions are unchanged.
- Rust (`rs/wrkslotsd`): the classification reader and the completed-record reader take the same drift-as-note change, progress is measured against the recorded hooks, classification JSON and human output match the Python edition, scoped skipping matches, and doctor reports the same finding. The Rust edition has no create-recovery writer (its recover writer implements only `--abort-import`, as recorded for #1337), so the abort and complete/resume paths are Python-only; parity for those paths is by the shared classification readers.
- Tests: Python lifecycle tests for drift on the abort path and the complete/resume path (the resume runs the journal's hooks, not the configuration's), drift as an audit note in classification and doctor, an unclassifiable journal blocking only its own slot while a second create succeeds, and the never-checked-out submodule shape at hook statuses completed, not-started, running and failed, with negatives for an index, a moved HEAD, local commits, a checked-out tree and authored files. A Rust differential test classifies a drifted journal in both editions, human and JSON.
