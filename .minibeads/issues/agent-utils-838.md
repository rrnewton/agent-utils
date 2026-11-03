---
title: 'archive-fetcher-upstream: excludes and agentcloud transport into the canonical log fetcher'
status: open
priority: 2
issue_type: feature
assignee: mu-validate@devbig030
labels:
- archive
created_at: 2026-11-03T23:44:56.003421017+00:00
updated_at: 2026-11-03T23:44:56.003421017+00:00
---

# Description

[opus 5.5] The agent-log archive runs its own copy of agent-utils' log fetcher, and the two have drifted: the archive copy gained an agentcloud node-exec transport (2026-10-26) and, on 2026-11-03, a default exclude list for credentials and databases; agent-utils' canonical copy has neither, while it has a later timeout split the archive copy lacks. The archive was re-copying a 14.6 GiB Codex history database on every run and holds key and auth-cache files it should never have copied.

# Design

Bring agent-utils' fetcher (and its tests) up to the archive copy's behaviour, then make the archive run the canonical copy again:
- DEFAULT_EXCLUDES applied to rsync and to the source manifest by the same matcher: auth.json, .credentials.json, *auth-cache*, *.key, *.token, *token*.json, /daemon/ directly under .claude, shell-snapshots/, *.sqlite, *.sqlite-wal, *.sqlite-shm; shown by --list; per-root excluded counts in the run log; excluding never deletes archived copies.
- The agentcloud transport (a 4th machines.tsv column, agentcloudctl node exec), with tests.
- Keep agent-utils' timeout split (_run_bounded / _run_bounded_bytes).
A ready patch for the excludes exists (applies to 9dcde0c3; its tests and mypy --strict pass): ~/agent_scratch/fetcher-excludes/upstream/agent-utils-excludes.patch with UPSTREAM-NOTE.txt on devbig030.

# Acceptance Criteria

- agent-utils' fetcher tests pass with new tests for the excludes (rsync and manifest) and the agentcloud transport; mypy --strict clean.
- The archive copy is replaced by (or diffed clean against) the canonical one.
