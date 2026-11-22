---
title: 'agentctl-token-redaction: revive and status JSON must never print an agent token'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- security
created_at: 2026-11-22T00:22:00-05:00
updated_at: 2026-11-22T00:22:00-05:00
---

# Description

A consumer saw an agentctl revive command print a lane's agentctl terminal token in its JSON output, visible in the lane's own console.

# Design

Redact every token field (record token, terminal token, any credential-like value) in revive, restore, status and every other JSON or text output by default, showing at most a short non-secret fingerprint; require an explicit opt-in flag for the raw value where a caller genuinely needs it, and never print it in logs. Audit every command's output for the same class.

# Acceptance Criteria

- No agentctl command prints a token by default; tests assert redaction for revive, restore and status output and for the event log.

# Notes

Filed by the coordinator from a consumer report; the owning lane refines this file as it works.
