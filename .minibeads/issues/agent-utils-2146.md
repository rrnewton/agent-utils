---
title: 'wrkslots-fenced-finish-strands-seal-on-busy-state-lock: a fenced validate finish refuses on a busy state lock and strands the batch seal'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-23T05:45:00-05:00
updated_at: 2026-11-23T05:45:00-05:00
---

# Description

A fenced `wrkslots remove --validate-complete` that finds the worktrees ACTIVE state lock busy REFUSES after the fence: the revalidate step refuses, the rollback needs the same lock and refuses too, and the caller exits. The interrupted validate-batch seal is kept while the finish journal is outstanding, so it blocks every validate admission and every create-journal classification on the host until a coordinator runs recover. A consuming team saw this four times in 24 hours; the lock holder can be any long wrkslots command (an audit, or anything else holding the state lock).

# Design

- After the fence, a finish must complete or roll back, never just exit: wait for the revalidate/rollback lock, bounded (about 600 s), while holding the writer-intent turnstile so new writers queue behind it, instead of refusing. Only when the bound expires refuse, leaving the journal for recover with a clear message.
- Waiting for a lock is not work under a lock; keep the work done once the lock is held constant-time (owner rule).
- Second part: shorten the audit's lock hold: snapshot under the lock, validate the event chain outside it, and re-check with an O(1) generation compare.

# Acceptance Criteria

- Test: a fenced finish meets a held state lock that is released within the bound -> the finish completes; held past the bound -> a clear refusal with the journal kept for recover; in neither case is a seal left without its finish journal.
- The audit's measured lock hold on a large registry drops, with the before and after numbers recorded.

# Notes

Reported by a consuming team (P1); filed by the coordinator. Owner: the lane working on #2125 (history read outside the config lock), as its next commit.
