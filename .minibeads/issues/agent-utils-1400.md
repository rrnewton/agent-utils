---
title: 'nested-foreign-checkout: wrkslots adopts an undeclared nested linked worktree of another repository instead of refusing write-handoff'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T17:05:44+00:00
updated_at: 2026-11-11T17:05:44+00:00
---

# Description

[opus 5.5] A consumer's slot holds a linked worktree of a second repository nested inside the slot directory, a repository the slot record doesn't declare. `wrkslots write-handoff` for that slot refused with exit 3 at generation 1:

    source repository <second-repo> registers unexpected path worktrees/slots/<slot>/<second-repo>-s5b inside target slot <slot>

The same finding stops `remove` and `retire`, and it bears on #1302 wrkslots-ignored-evidence-salvage, which runs on the same remove path. The slot was left untouched and the handoff was kept as a plain file.

# Design

The lead's ruling (2026-11-11):

1. **remove and retire keep refusing.** An undeclared nested checkout is never deleted silently.
2. **write-handoff does not refuse.** Writing a handoff is harmless, so there the finding is a loud warning, and the handoff records it.
3. **A deterministic remedy, named in the refusal text:** a command such as `wrkslots adopt-checkout SLOT --repo <name>=<path>` (exact form to fit the CLI). It declares the nested linked worktree into the slot record after verifying that it is a clean linked worktree of a known repository. After that, remove salvages and archives it like any declared checkout, which keeps the slot TTL design's rescue deterministic. The refusal also names `git worktree move` out of the slot as the manual alternative.
4. **The Rust edition gets the same handling,** or a parity note says which edition owns it.

# Acceptance Criteria

- write-handoff on a slot with an undeclared nested linked worktree succeeds, prints a warning naming the path and repository, and the handoff records it.
- remove and retire still refuse, and the refusal names the adopt-checkout command and `git worktree move`.
- adopt-checkout refuses a dirty worktree, a path that isn't a linked worktree, and an unknown repository. On success the checkout appears in the slot record, and a later remove salvages it (Git history plus #1302's ignored-file handling) and archives it.
- Fixture tests only (never a real consumer tree); Python and Rust parity or a parity note.

# Notes

Related: #1302 wrkslots-ignored-evidence-salvage (same remove path). Numbered from devbig030's interim block (1400-1469).

Observed 2026-11-12 on devbig030: `wrkslots heartbeat` refuses the same way for a slot holding a nested worktree (there, mu-ignsalv's own `.muse/worktrees/ign1400` design worktree inside slot ignsalv):

    wrkslotsd: source repository /home/newton/work/dev-agent-utils/agent-utils (Git common directory /home/newton/work/dev-agent-utils/agent-utils/.git) registers /home/newton/work/dev-agent-utils/worktrees/slots/ignsalv/.muse/worktrees/ign1400 inside the managed agent root, but no active checkout row for that repository names it

Heartbeat is the same gap on another surface, but it is **out of scope here**: #1492 wrkslots-heartbeat-refuses-nested-worktree (with P0 #1491) makes heartbeat warn and record instead of refusing. This note stays only as a cross-reference to #1492; #1400 covers remove, retire and write-handoff. The slot was left as it is; nothing was moved or forced.
