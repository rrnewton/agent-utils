---
title: 'agentctl-shepherd-review7-followups: deferred shepherd hardening from the seventh cold review'
status: open
priority: 2
issue_type: task
labels:
- agentctl
created_at: 2026-11-24T06:30:00-05:00
updated_at: 2026-11-24T06:30:00-05:00
---

# Description

Deferred should-fix items from the seventh cold review of #1559 restore-recovery-agent-plan, deliberately not folded into that landing: page retry backoff matching the driver's 60/300/900 ladder, transient heartbeat error handling beyond three retries, an act-expiry test and operator doc for the 30-minute queue expiry, and remaining stale shepherd comments. Each item lands with its own focused test.

# Design

- Retry ladder: page retries follow 60/300/900 seconds like the driver escalation path, bounded by the incident.
- Heartbeat: transient claim errors distinguish a broken state dir (page and exit) from a momentary read failure (keep supervising).
- Act expiry: a test proves a queued request older than the expiry is refused as expired at execution, and the userguide states the bound.
- Comments: sweep remaining shepherd comments for pre-rework behaviour.

# Acceptance Criteria

- Each item has a focused failing-first test or a named file:line proof.
- No behaviour change to the landed claim, page, or epoch semantics without a new design check.

# Notes

Filed from the #1559 lane at coordb's direction during the seventh cold review round; the blocker and correctness items of that review were fixed in the same round, and these remainders were explicitly deferred here.
