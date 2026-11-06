---
title: 'wrkslots-recover-ownerless-agent-remove: generic recover and audit refuse an ownerless-agent-remove journal'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-06T18:34:21.000000000+00:00
updated_at: 2026-11-06T18:34:21.000000000+00:00
---

# Description

Two refusals for the `ownerless-agent-remove` recovery journal kind, both seen on a consumer host:

1. Generic `wrkslots recover` cannot resume an ownerless-agent-remove journal through the wrapper's host service. It refuses with "coordinator PID ... is not in the invoking process ancestry". `_cmd_recover` dispatches ownerless-agent-remove to `_recover_ownerless_agent(config, path, raw, coordinator)` without the captured `_RecoveryProcesses`, and `_recover_ownerless_agent` begins with a direct ancestry assert. That assert cannot hold when the host service runs the command. The kind-specific `recover-ownerless-agent-worktree --apply --coordinator-authorized` works.
2. `wrkslots audit --format json` refuses with "unsupported cache recovery journal kind ownerless-agent-remove", so one pending journal makes the whole audit unreadable.

# Design

- Pass the captured recovery processes through the generic dispatch, the way the other journal kinds get them, and check the authorization against those instead of the live ancestry.
- Teach audit's journal reader the ownerless-agent-remove kind (and check whether any other journal kind is missing from it), so audit reports the journal instead of refusing.
- Keep the Rust edition in step if it reads the same journals.

# Acceptance Criteria

- A test: generic recover, run through the host-service path, resumes an ownerless-agent-remove journal.
- A test: audit --format json with a pending ownerless-agent-remove journal succeeds and lists it.
- The kind-specific command keeps working.

# Notes

Reported 2026-11-06 by the lead coordinator from a consumer's coordinator. Queued for mu-wrkslots after its pending #864 stack.
