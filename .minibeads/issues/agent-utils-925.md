---
title: 'revive-replay-queue: opt-in delivery of the archived queue after a revive'
status: open
priority: 3
issue_type: feature
created_at: 2026-11-06T05:40:00.000000000+00:00
updated_at: 2026-11-06T05:40:00.000000000+00:00
---

# Description

`agentctl revive` archives the agent's old prompt queue instead of replaying it, including when it abandons a dead candidate (#899 revive-abandon-dead-candidate). That is the safe default, because replaying a stale queue can deliver out-of-date or duplicate prompts. Some operators may still want the archived prompts delivered after a successful revive.

# Design

Add `revive NAME --replay-queue`. After the revived session is verified ready, deliver the archived prompts in their original order through the normal admission and receipt path, skipping any prompt that has expired or that was already delivered (by message id). Print what was replayed and what was skipped.

# Acceptance Criteria

- Without the flag, behaviour is unchanged: archived, reported and not delivered.
- With the flag, archived prompts are delivered once, in order, after readiness. Expired and already-delivered prompts are skipped, and each skip is reported.
- Tests cover order, dedupe by id, and expiry.

# Notes

Filed 2026-11-06 12:40 AM ET at the lead's request; no work now.
