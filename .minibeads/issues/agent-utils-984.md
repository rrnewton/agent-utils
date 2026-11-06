---
title: 'wrkslots-root-through-symlink: check that wrkslots called through a symlinked launcher still finds the harness root'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-06T22:36:35.000000000+00:00
updated_at: 2026-11-06T22:36:35.000000000+00:00
---

# Description

A consumer calls wrkslots through a `bin/wrkslots` symlink. From restarted lanes, its root resolves to the parent work directory instead of the harness, so transfer-owner refuses. The workaround is to call it by absolute path.

The Python edition's `_discover_root` searches from the current directory upward for `.wrkslots.yml`, not from the launcher's location, so the Python CLI alone should not depend on the symlink. Check the other paths: the `common/bin/wrkslots` launcher, the Rust `wrkslotsd` edition, and the soak wrapper. Each must resolve the root the same way, by `--root`, then the cwd upward, never from the resolved location of the executable.

# Acceptance Criteria

- Tests run through a symlinked launcher from a slot directory: every edition finds the harness root.
- A fix, if any edition resolves from the executable's path.

# Notes

Reported 2026-11-06 by the lead coordinator from a consumer lane.
