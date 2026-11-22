---
title: 'agentctl-goal-busy-claude: agentctl goal times out on a Claude lane that is mid-turn'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-22T00:05:00-05:00
updated_at: 2026-11-22T00:05:00-05:00
---

# Description

A consumer reports that `agentctl goal NAME --file F` times out (rc 124 after 150 s) for Claude Code lanes that are mid-turn, three tries each on three lanes; Muse lanes take goals fine. The command appears to wait for an idle composer without queueing.

# Design

Queue the goal durably when the lane is busy (as `send --deliver-while-working` does) and deliver it at the next safe point, or fail fast with a clear reason and a next step; never hang until a timeout. Match the behaviour across harnesses (see the harness support matrix).

# Acceptance Criteria

- `goal` against a busy Claude lane returns promptly: queued (and later delivered) or refused with a reason; deterministic tests with a fake busy pane; the harness support matrix updated.

# Notes

Filed by the coordinator from a consumer report; the owning lane refines this file as it works.
