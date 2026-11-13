---
title: 'wrkslots-heartbeat-refuses-nested-worktree: heartbeat refuses on git-registration-without-row for its own slot'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
- heartbeat
created_at: 2026-11-13T15:00:00+00:00
updated_at: 2026-11-13T15:00:00+00:00
---

# Description

[watermelon-full-preview] `wrkslots heartbeat SLOT` refuses when the slot contains a nested worktree that Git registers but no checkout row names (`git-registration-without-row`), e.g. `<slot>/.muse/worktrees/<name>` or `<slot>/ignored/<name>`. Lanes routinely keep nested worktrees there, so a live owner cannot keep its slot off the TTL ladder (see #1491). Elsewhere the same finding is a warning (e.g. `hold` on another slot succeeds with a warning; `transfer-owner` tolerates it).

# Design

Strictly heartbeat: warn and record, never refuse on that finding. Python `_cmd_heartbeat` passes `tolerate_target_git_registrations=True` to `_assert_command_registry_storage` (call-site only) and prints each tolerated detail as a `WARNING` on stderr while still recording the heartbeat; Rust `heartbeat_write.rs` uses `refuse_target_storage_except_nested` and `eprintln!`s the same warning. Do NOT change how `remove`/`retire`/`write-handoff` treat nested worktrees. Do NOT touch the shared detection (`_assert_registry_storage_consistent` / `parse_storage`) without telling the coordinator first. Converges with #1400 nested-foreign-checkout (same `git-registration-without-row` finding for write-handoff/remove/retire on another lane/host).

# Acceptance Criteria

- Slot with a registered nested worktree (e.g. `ignored/<name>`) -> `heartbeat` succeeds, heartbeat is recorded (revision/heartbeat_at advances), and a warning naming the nested path is emitted.
- No nested worktree -> heartbeat behaviour unchanged.
- `remove`/`retire`/`write-handoff` nested handling unchanged.
- Scratch project roots only.

# Notes

Related: #1491 TTL ladder, #1400 nested-foreign-checkout. Reference #1400 so the two converge on the same finding.

## Resolution

[watermelon-full-preview] Heartbeat only: Python _cmd_heartbeat tolerates the target git-registration-without-row via _assert_command_registry_storage(tolerate_target_git_registrations=True) and warns on stderr while recording; Rust heartbeat_write uses refuse_target_storage_except_nested with the same warning. No change to remove/retire/write-handoff nested handling or to shared detection; converges with #1400. Test test_heartbeat_succeeds_with_warning_for_nested_worktree in test_ttl_guard.py (real nested worktree under ignored/).
