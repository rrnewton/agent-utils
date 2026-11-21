---
title: 'hot-host-census-tests: wrkslots census and agentctl revive tests fail on a busy host'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- agentctl
- testing
created_at: 2026-11-21T12:45:00-05:00
updated_at: 2026-11-21T12:45:00-05:00
---

# Description

On 2026-11-21 every full gate of a tree touching wrkslots or agentctl went red in steps its change does not touch, on different shards each time, because the tests depend on the real host process table:

- same-UID census that never closes while unrelated processes fork and exit (see #1957);
- Rust/Python census differentials that disagree when an exiting process has an unreadable fd directory (Rust says indeterminate, Python says deletable);
- agentctl revive and dead-adoption tests that fail closed when any unrelated same-UID non-dumpable process exists on the host during the run;
- shards that hit their CPU limit under load.

Each named failure passes when run alone. The owner's rule is that every gating check passes on a host above 75% CPU and nothing time- or host-churn-sensitive gates.

# Design

Fast first: quarantine the affected cases out of gating, each with a pointer to this issue. Then make them hermetic: drive the census and the holder scan from a controlled process inventory (a fake proc root or an injected inventory) instead of the live host table, so the tests check the logic deterministically. Keep at most one opt-in real-host smoke test outside the gate.

# Acceptance Criteria

- No gating wrkslots or agentctl test reads the live host process table for its verdict.
- The census-not-closed, exiting-process and non-dumpable-holder cases are each covered by deterministic tests on a controlled inventory.
- The quarantine is removed once the hermetic versions pass under the gate's load.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
