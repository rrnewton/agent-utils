---
title: 'watch-blocked-fast-alerts: report a lane blocked at an approval prompt or guardrail refusal on the next pass, with a fast prompt-only check'
status: closed
priority: 1
issue_type: feature
labels:
- agentctl
- watch-agents
created_at: 2026-10-28T20:16:45.653757837+00:00
updated_at: 2026-10-28T22:20:03.335373154+00:00
closed_at: 2026-10-28T22:20:03.335372964+00:00
---

# Description

[opus 5.5] Owner request (relayed 2026-10-28): unlike idleness, a blocked agent can be reported at once. Two lanes on another registry sat at approval prompts about 10 h unnoticed.

1. A lane at a harness approval or permission prompt, or showing a guardrail refusal, is notified on the next pass, without the --approval-minutes wait, and without waiting for --min-notice-interval.
2. A separate, faster cadence (e.g. 60 s) for the cheap blocked-prompt screen check only (visible rows), while idle and stall detection keep their thresholds and the full pass keeps its interval (900 s on one registry).

Share the pattern table with #665 bridge-stuck-agent-alerts. Builds on #669 watch-notice-delivery.
