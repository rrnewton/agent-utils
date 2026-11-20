---
title: 'wrkslots-init-command-wrapper: point the init command symlink at the real-interpreter wrapper'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
- launcher
created_at: 2026-11-20T00:00:00-05:00
updated_at: 2026-11-20T00:00:00-05:00
---

# Description

[watermelon-full-preview] Follow-up to #1964 wrkslots-full-status-hang-ignores-sigterm. `wrkslots init` creates the project-local command symlink (control/wrkslots) pointing at the package `__main__.py`. Executed directly, that file's `#!/usr/bin/env python3` shebang goes through the host's fbpython launcher again — the exact launcher whose SIGTERM-catching wait stranded a full status in #1964. Fresh projects should get a command that runs on the real interpreter directly, as invocations through the tracked dispatcher (`common/bin/wrkslots`, whose Python leg execs the resolved interpreter on the entry script) now do.

# Design

- Fresh init links point at the tracked Python entry script (py/bin/wrkslots from the source checkout), falling back to `__main__.py` where no such entry exists (an installed package).
- Accept the old `__main__.py` target as a known-migratable previous target under `--repair`, alongside the existing single-file previous target: a repair rewrites the link to the entry script instead of refusing.
- A non-repair re-init whose link points at the old target keeps refusing by name, so no silent retargeting happens outside the repair path.

# Acceptance Criteria

- A fresh `init` in a scratch project leaves control/wrkslots pointing at the entry script, and invoking it through the dispatcher runs on the real interpreter (no launcher parent).
- `init --repair` on a project whose link still points at `__main__.py` migrates the link to the entry script and reports the repair.
- A plain re-init on such a project refuses and names the repair remedy.
- Lifecycle tests cover fresh init, re-init refusal, and repair migration.

# Notes

Issue-only for now (#1964 landing); implementation is a separate change. No persisted fields: the symlink lives in tool-owned control state and its target is re-derived at init/repair time.
