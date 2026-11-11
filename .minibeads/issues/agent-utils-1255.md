---
title: 'wrkslots-private-history-refusal: removal names unpushed private submodule history instead of calling it uncommitted changes'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T00:50:00+00:00
updated_at: 2026-11-11T00:50:00+00:00
---

# Description

[opus 5.5] A consumer's slot retirement is blocked by a misleading refusal. Its checkout holds a self-sourced submodule that is clean: no status, diff or untracked lines. But the submodule has commits that aren't proven durable (in the reported case, 483 refs and 52 commits off HEAD that no remote is known to hold). `wrkslots remove` treats the unproven private history as dirty and refuses with "submodule NAME has uncommitted changes and its source no longer declares it; preserve the slot and commit or publish that work by hand".

The refusal is correct, since the history may be the only copy. The message is wrong, though. The operator looks for uncommitted changes, finds none, and has no documented way forward.

# Design

- Keep the two facts apart in the salvage facts: uncommitted work (status, diff, untracked) versus unproven private history (commits or refs not shown to exist on any remote and not bundled).
- When removal refuses on private history, say what it found: "unpushed private history in submodule NAME: N commits on M refs not on any remote", list a few of them, and name the command that preserves them.
- Document a salvage path: a way to preserve that history before removal, for example pushing it to a backup ref under `refs/rescue/` on a configured remote, or bundling it into the slot's salvage archive as clean submodules already are (#871). After that, removal proceeds without hand edits. The operator runs it explicitly; removal never pushes on its own.
- Rust parity: the Rust edition's refusal text and facts match.

# Acceptance Criteria

- A clean self-sourced submodule with unproven private history refuses with a message naming private history and its counts, never "uncommitted changes".
- A submodule with real uncommitted work still refuses with the uncommitted-changes message.
- The documented salvage step preserves the history, and a following removal succeeds. Tests cover both, in both editions.

# Notes

Reported by a consumer team on 2026-11-10 (redacted). The refusal must stay; only the diagnosis and the way forward change.
