---
title: 'team-tick-hub-follow-through: a tick-hub for the agent-utils team that flags stalled work from existing facts'
status: open
priority: 1
issue_type: task
labels:
- coordinator
created_at: 2026-11-22T10:20:00-05:00
updated_at: 2026-11-22T10:20:00-05:00
---

# Description

This team runs no tick-hub, so stalled items (an owner fast-track gate waiting on a known-red rerun, landed changes never deployed, a failed turnover) go unnoticed for hours.

# Design

Every 15 minutes, fact-only checks to the coordinator: owner-tier gate rows whose only reds are on the known-red list, and owner-tier reruns queued over 30 minutes; landed vibe-talk commits not live after 2 h, and services whose snapshot lacks landed changes after a failed turnover; failed team timer units; steps that failed on 5 or more unrelated changes in 7 days; a daily 8 PM follow-through prompt with recent owner prompts; owner ETAs past due.

# Acceptance Criteria

- The checks run on a timer, emit parseable ACTION lines, and each has a test with fixture facts.

# Notes

Follow-up from the #2105 follow-through report; filed by the coordinator.
