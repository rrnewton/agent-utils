---
title: 'agentctl-start-codex-0164-readiness: detect readiness for the Codex v0.164.0 composer'
status: closed
priority: 1
issue_type: bug
assignee: cl-mq@devbig030
labels:
- agentctl
created_at: 2026-11-03T08:45:50.745201925+00:00
updated_at: 2026-11-03T13:02:55.605275455+00:00
closed_at: 2026-11-03T13:02:55.605275295+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator (P1). `agentctl start` cannot detect readiness for Codex v0.164.0: Herdr reports the agent as "unknown" with no readiness, so startup times out twice even though the composer is up. The workaround on a consumer team's host was retire-stale-registration, a fresh start, `herdr agent rename`, and `agentctl promote`.

# Design

- A readiness probe that recognises the Codex v0.164.0 composer (and does not depend solely on Herdr's agent detection, which reports "unknown" for it).
- The startup timeout error names the observed state (Herdr's agent kind and status, and what the screen showed), so the next unrecognised version is diagnosable without a workaround.

# Acceptance Criteria

- `agentctl start` with a Codex v0.164.0 harness reaches ready without timing out (fixture from a captured screen).
- A start that does time out reports the observed agent kind, status and screen evidence.
