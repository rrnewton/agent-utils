---
title: 'wrkslots-remove-proof-writer-pipe-lost-at-boundary: a long remove loses its handoff proof writer before the locked boundary'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T15:45:41-05:00
updated_at: 2026-11-28T14:20:10-05:00
---

# Description

[watermelon-full-preview] Field report (2026-11-16): a `remove` of an agent slot with private history to bundle ran more than 20 minutes, then refused "removal proof writer no longer holds the handoff pipe at the locked removal boundary" (first attempt). The removal did all of its slow salvage work and then refused at the boundary for a reason unrelated to the slot.

The check is `_assert_remove_processes` (`py/wrkslots/cli.py`): under the mutation locks it requires the host-context proof writer captured by `_capture_remove_processes` to still appear in `_pipe_write_holders(proof_fd)` and to still descend from the coordinator. Something in a 20-minute remove lets that writer go: a timeout or budget kill of the writer process, the pipe's read end closing, or the wait for the registry lock itself (the long hold #1735 wrkslots-remove-holds-state-lock-through-salvage removes, though this removal was the one waiting, not holding).

# Design

- Reproduce first: name which mechanism drops the writer (writer process exited, pipe inode changed, holder scan missing it), with evidence from the code paths that can kill, close, or time out the writer or the pipe during salvage and during the lock wait.
- Fix that mechanism so a removal that was properly authorized at capture stays authorized at the boundary, without weakening the proof: a writer that genuinely exited or was replaced must still refuse.
- Or, if the long lock wait is the mechanism and the #1735 lock narrowing removes it, show that with the same reproduction and say so here.

# Acceptance Criteria

- A deterministic regression test for the named mechanism: a remove whose salvage (or lock wait) is artificially slow, via an event or barrier stub and never a wall-clock budget, still passes the locked-boundary proof-writer check.
- A test that a proof writer that really exited before the boundary still refuses with the same message.
- Python and Rust editions agree where the Rust edition has the path (host-context removal currently stays with the Python edition).

# Notes

Related: #1735 wrkslots-remove-holds-state-lock-through-salvage (the lock wait this refusal may be a symptom of). #1735 has since landed (fc848a75, closed 2026-11-21), so the first question is whether this refusal still reproduces on current main.

Filed 2026-11-28 from an issue file drafted on 2026-11-16 that never reached main (found during worktree cleanup). The code references above were re-checked against main on 2026-11-28: `_assert_remove_processes`, `_capture_remove_processes` and `_pipe_write_holders` are still in `py/wrkslots/cli.py` with the same refusal text, and host-context removal still has no Rust edition.
