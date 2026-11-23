---
title: 'herdr-server-log-rotation-loses-evidence: the herdr server log keeps no rotated copy, so pane kills cannot be attributed'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-23T06:15:00-05:00
updated_at: 2026-11-23T06:15:00-05:00
---

# Description

A consuming team's interactive agent pane was SIGKILLed at a known minute; the herdr server had been running for days, but its log had been rotated about 30 minutes later and kept no older copy, and pane close and kill events do not record their cause, so the kill could not be attributed to Herdr or ruled out.

# Design

Keep at least one rotated copy of the server and client logs (size-based, bounded total), and log every pane close or kill with its cause (client request with request id, process exit with status and signal, server-initiated cleanup with reason).

# Acceptance Criteria

- After a rotation the previous log is still present; a test closes and kills panes and finds one log line per event naming its cause.

# Notes

Reported by a consuming team after the morning services turnover; filed by the coordinator.
