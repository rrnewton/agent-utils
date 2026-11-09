---
title: 'inbox-backpressure-design: per-sender cool-down on upward lane-to-coordinator sends, designed from telemetry'
status: open
priority: 2
issue_type: task
assignee: mu-inbox
labels:
- agentctl
created_at: 2026-11-09T17:05:54.262818602+00:00
updated_at: 2026-11-09T17:14:32.841374158+00:00
---

# Description

DESIGN ONLY - needs lead coordinator and owner approval before any implementation. Owner request 2026-11-09 (reply 251): track inbound messages/minute per coordinator; when too high it signals tick-hub configured too aggressively or too many chatty direct reports. If individual sub-agents message multiple times in quick succession before the coordinator responds, consider cool-down / back-pressure on individual upward lane-to-coordinator links: agentctl send could fail with a reminder that the coordinator may be busy, to keep working autonomously and message again only after X minutes. Decisions must be data-driven from queue telemetry.

Design questions to answer from the 2026-11-09 coordinator inbox telemetry (harness report): what per-sender unanswered-burst threshold separates real floods from normal DONE/BLOCKED reports; cool-down length; refusal semantics (exit code, message text, whether the refused text is stored anywhere - it must not be silently lost); exemptions (owner messages via bridge always pass; automated notices carrying TTL + supersede key always pass - they self-limit); scope (only upward lane-to-designated-coordinator sends?) and configurability per registry. Include the failure mode where back-pressure hides a genuine urgent BLOCKED report, and how the design avoids it.

# Acceptance Criteria

- A design recorded on this issue (or a linked ai_docs file) with thresholds derived from measured telemetry, not guesses: per-sender burst definition, cool-down, refusal contract, exemptions (owner; TTL+supersede automation), and urgent-report safety.
- Explicit owner/lead approval recorded before implementation is filed as its own issue. No code lands under this issue.
