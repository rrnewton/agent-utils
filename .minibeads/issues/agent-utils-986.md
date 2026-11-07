---
title: 'restart-when-idle: restart a live agent in its own pane with its recorded session'
status: open
priority: 0
issue_type: feature
created_at: 2026-11-06T21:04:18.359346800+00:00
updated_at: 2026-11-06T23:38:32.087131861+00:00
---

# Description

R1 (owner reply 174): agentctl restart NAME --when-idle waits for an idle turn with nothing queued, exits the harness gracefully (Claude /exit with background-dialog abort; Muse /exit then Ctrl-C twice), relaunches in the same pane/tab resuming the recorded session, re-pins, restores the Herdr name, transfers wrkslots slot ownership, and queues a re-arm notice. Refuses cleanly when the session is unknown/contradictory. Lessons in ai_docs/transient/2026-11-06-restore-in-place/LESSONS.md.

# Edition parity note (Python)

Same-pane reuse and `revive --fresh` are Rust-edition behaviour. The Python edition's `restart` relaunches in a fresh tab and closes the old pane, and its `revive` has no `--fresh` flag (refused by name, pinned by `test_revive_fresh_is_a_rust_only_flag_in_this_edition`). The cross-environment differential has no `restart` group, and its `revive` interop group uses native `herdr` records, for which the Rust edition also opens a fresh tab, so the divergence is outside the differential's asserted surface. Python `restart`/`revive` session resolution covers the recorded session and the foreground-argv probe; the Muse `session.jsonl` discovery exists in Python revive.
