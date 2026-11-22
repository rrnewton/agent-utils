---
title: 'watch-agents-waiting-verdict: watch-agents understands a WAITING verdict instead of treating every wait as done'
status: open
priority: 2
issue_type: feature
labels:
- coordinator
created_at: 2026-11-22T10:20:00-05:00
updated_at: 2026-11-22T10:20:00-05:00
---

# Description

Most child-done notices this week were lanes waiting (on gates, reruns or review), which drowns real completions.

# Design

Accept a WAITING verdict with a state and since-time; notices name the item and state; repeated identical waits are not re-notified; a WAITING past its state stall timer is escalated.

# Acceptance Criteria

- Tests: a WAITING turn raises no child-done notice; a stale WAITING escalates once.

# Notes

Follow-up from the #2105 follow-through report; filed by the coordinator.
