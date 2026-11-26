---
title: 'wrkslots-supersede-handoff-stale-task-scope: supersede-handoff keeps the old task_scope for the new owner'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-25T07:00:00-05:00
updated_at: 2026-11-25T07:00:00-05:00
---

# Description

supersede-handoff replaces a slot's owner but keeps the old owner's task_scope. On a task-scoped slot the appended active-record event then refuses to load: "append-only active-record event.record.task_scope does not match its bound owner process" — the same refusal #2179 fixed for transfer-owner (landed c98e82e4), reached here through a different command.

# Design

Give supersede-handoff the same treatment transfer-owner received in #2179: when the owner is replaced, rebind task_scope to the new owner's verified scope (derived from the successor's live process and verified through the systemd runtime invocation link), or drop the scope when the successor has no verifiable task scope, so the record passes the loader. Unlike transfer-owner there is no dead-owner precondition: supersede replaces a live handoff, so the rebind or drop happens whenever the owner changes. Python and Rust (wrkslotsd) parity. No new persisted fields are expected.

# Acceptance Criteria

- supersede-handoff on a task-scoped slot succeeds and the resulting record passes the registry loader.
- The new record's task_scope, when kept, is bound to the new owner; when the successor has no verifiable scope, the scope is dropped rather than left stale.
- Python and Rust behave identically, covered by a deterministic regression test (no wall clock, no root).

# Notes

Found by inspection while #2179 was fixed for transfer-owner: the two commands replace a slot's owner through different paths, and only transfer-owner learned to rebind the scope. The scope's bound-owner check itself is unchanged and still refuses a genuinely mismatched scope.
