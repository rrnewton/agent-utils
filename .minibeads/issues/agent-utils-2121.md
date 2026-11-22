---
title: 'agentctl-watch-hooks-trust-missing-file: a registry without hooks/agent.json means no hooks to trust, not an error'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-22T11:28:00-05:00
updated_at: 2026-11-22T11:28:00-05:00
---

# Description

agentctl watch reports "hooks_trust ...: cannot read JSON <registry>/hooks/agent.json: No such file or directory" for a registry whose hooks directory never had that file (it holds only a permission hook script), with the profile's Muse hooks trust set to trust. The check treats a file most registries do not have as an error every pass.

# Design

A missing hooks agent file reads as "no hooks to trust" and raises nothing; only an unreadable or malformed existing file is a problem. If the file is meant to exist, agentctl writes it and documents its schema; no operator hand-writes it.

# Acceptance Criteria

- Tests: a registry without the file produces no watch problem; a malformed file still reports one; the schema, if any, is documented.

# Notes

Reported by a consumer's lead; filed by the coordinator.
