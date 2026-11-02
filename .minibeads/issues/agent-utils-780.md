---
title: 'wrkslots-retire-freeze: a per-host switch that refuses every slot retirement and removal'
status: open
priority: 0
assignee: mu-rust@devbig030
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-02T17:30:03.603445311+00:00
updated_at: 2026-11-02T17:30:03.603445311+00:00
---

# Description

[opus 5.5] Owner order 2026-11-02: do not delete or move worktrees on one host while a filesystem isolation migration is planned. A consumer team's validate cleanup worker ran 'wrkslots retire-pending --limit 8 --coordinator-authorized' unconditionally after every drain. Spec: (1) a per-host switch: 'retire_freeze: <reason>' in the wrkslots config and/or a marker file under the wrkslots control dir; (2) while set, retire-pending, remove, finish-with-removal and clean-caches REFUSE with a stable reason code and the freeze reason, for every caller including --coordinator-authorized; (3) read-only commands (status, audit, unpushed, ...) keep working; (4) 'wrkslots freeze set <reason>|clear|status' manages it; (5) default off; (6) tests in both editions where the command exists (Python and the Rust edition under soak), plus docs/help.
