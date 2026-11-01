---
title: 'agentctl-test-host-index: agentctl tests write coordinator host-index entries into the real per-user state directory'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-01T13:06:31.829132121+00:00
updated_at: 2026-11-01T13:06:31.829132121+00:00
---

# Description

[claude-opus-5-5] Found while fixing #721 agent-teams-coordinator-handover. Every agentctl test that calls coordinator set (subagents/tests/control.rs and others) refreshes the per-host coordinator index at host_index_dir(), which defaults to ~/.local/state/agentctl/coordinators because the test Fixture sets neither AGENTCTL_COORDINATOR_INDEX_DIR nor XDG_STATE_HOME. On the development host 769 of 771 index files there name /tmp/agentctl-managed-* fixture registries left by test runs; `agent-teams coordinators` reads that directory. Wanted: tests use a per-fixture index directory (an injectable path on ManagedAgents, not a process-wide env var, which races between tests), and a one-off, dry-run-by-default cleanup of index entries whose registry no longer exists.
