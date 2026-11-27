---
title: 'wrkslots-submodule-repo-create-flips-worktreeconfig: a slot created under one submodule repository can still deliver the broken worktree-config state'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-26T19:41:00-05:00
updated_at: 2026-11-26T19:41:00-05:00
---

# Description

Reported by a trusted consumer coordinator (2026-11-20, re-checked 2026-11-26): a validate-fresh create on one submodule repository sets `extensions.worktreeConfig=true` in the SHARED submodule git dir while `core.worktree` is still in its common config. The fresh checkout is then broken (phantom mass deletions on status/diff), and the consumer currently forbids same-repo submodule slots because of it.

What is already covered, so this issue is only the remainder:

- `#1096` introduced the identity writer that flips the extension, then described as inert.
- `#1435` (closed 2026-11-16) made wrkslots' own writers safe — the identity path refuses rather than flip while `core.worktree` is in the common config, the shared-submodule preparation moves the value into `config.worktree`, and doctor reports the broken state (`worktree-config-core-worktree-in-common`) after the fact. Its Notes explicitly leave a second writer unchanged: a consumer post-provision hook of the form `git config extensions.worktreeConfig true`.
- `#1206` is the shared-submodule-objects work whose preparation moves `core.worktree` correctly. `#1436` (create-journal hook drift) is unrelated.

The uncovered remainder is the create outcome: when create runs on a repository that is itself a submodule (its git dir is a module git dir whose common config carries `core.worktree`), a hook can still flip the extension in that shared git dir and wrkslots delivers the slot in the broken state #1435's doctor finding only later names. The consumer's workaround — forbidding same-repo submodule slots outright — is the cost of that gap.

# Design

Close the gap at create time, in the same single-writer spirit as #1435:

- After the post-provision hooks and the identity step, verify the worktree-config state of every repository the create touched (slot root checkout and each populated submodule repository) with the existing single reader (`_inspect_worktree_config`): extension on while `core.worktree` remains in the common config is the broken state.
- A create that would deliver that state refuses at create with the same remedy wording as #1435 (move the value into the main worktree's `config.worktree`, or unset the extension), naming the repository and the state found — or, if a repair inside create is judged safe for a repository wrkslots itself just made, performs the verified move and logs it. Refusal is the conservative default; repair only where #1435's move-mode justification already applies.
- The check runs on the recovery path too (a recovered create re-runs hooks), and recovery of an already-broken slot reports the state rather than re-verifying blind.
- Python and Rust editions behave the same where each has the path; where an edition lacks it, the Resolution says so, as #1435's did.

# Acceptance Criteria

- A create whose hooks flip `extensions.worktreeConfig` in a submodule repository whose common config holds `core.worktree` does not deliver a slot in the broken state: it refuses (or performs the verified move) naming the repository, and `git status` and `git rev-parse --show-toplevel` in the primary submodule checkout and a sibling are unchanged afterwards.
- A create on the same shape with no such flip behaves exactly as before (identity, hooks and registration unchanged).
- The doctor finding from #1435 still fires on a state created outside wrkslots, and does not fire on slots this create delivered.

# Notes

Checked before filing at the coordinator's request (2026-11-26): #1096 (the writer's origin), #1206 (shared submodule objects and its correct preparation), #1435 (wrkslots' own writers made safe plus after-the-fact doctor detection; its Notes name the consumer-side hook as a second writer left unchanged), #1436 (create-journal hook drift, unrelated). None covers create delivering the broken state when the flip comes from the create's own hooks on a submodule repository, which is the still-real report above. Related: `#2307 test-fakes-unread-post-body-keepalive-poison` (same reporting pass, different mechanism).
