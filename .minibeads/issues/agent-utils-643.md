---
title: 'agentctl-test-process-leak: agent_watch test binaries outlive the gate and block slot removal'
status: open
priority: 2
issue_type: task
labels:
- agentctl
- validation
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:58:47.632484469+00:00
updated_at: 2026-10-28T13:58:47.632484469+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (entry dated 2026-10-28). After a gate, 13 rust agentctl test processes from the agent_watch scenario tests (target/debug/deps/agentctl-*) were still running 40 minutes later with their cwd in the slot, which blocked removing the slot. Wanted: find the test that spawns a child and never reaps it, make it kill and wait in every exit path, and add a check that the test binary leaves no children.
