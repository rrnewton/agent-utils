---
title: 'clean-caches-skip-running-gate: never delete build output from a slot whose gate is running'
status: closed
priority: 1
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-07T22:50:00+00:00
updated_at: 2026-11-08T12:36:13.623509856+00:00
closed_at: 2026-11-08T12:36:13.623509746+00:00
---

# Description

The owner's standing rule (2026-11-07): regenerable build output (cargo targets, page build trees) may be deleted from ANY slot, whether idle, finished or active, without asking, EXCEPT a slot whose gate or validate is running at that moment. Deleting a running gate's target makes its build or test steps fail, or makes them rebuild under memory caps.

`wrkslots clean-caches --yes` removes configured caches from every unheld active or unregistered slot without checking for a running gate. Today an operator has to find the running gates by hand (gate admission runs/ and queue/, or processes whose cwd is inside the slot) and pass `--only` for the rest.

# Design

- Before deleting a slot's caches, detect a running gate in it. Check the gate admission state (`$GATE_ADMISSION_DIR`, default /tmp/dev-agent-utils-gate-admission: runs/*.json and queue/*.json name a pid whose cwd is in the slot), and fall back to any live dagrun/validate.py/cargo process whose cwd is inside the slot.
- Such a slot is SKIPPED with a reason (`SKIPPED: <slot> gate running (pid N)`), for `--yes` and for `--only`. `--force-running` overrides and is recorded.
- A queued (not admitted) ticket counts as running by default, because its gate will build into the target soon. Say so in help.
- The report shows reclaimable bytes for skipped slots so the operator can come back.

# Acceptance Criteria

- A test with a fake admission run in a slot: `--yes` skips that slot, cleans the others, and names the reason.
- `--only` on the running slot refuses without `--force-running`.
- Help documents the rule and the override.

# Notes

Found during the 2026-11-07 stale-slot cleanup: an operator cleaned four idle slots with --only (55.6 GiB) and had to skip six slots with running or queued gates by hand.
