---
title: 'agentctl-queue-only-drain: deliver queued prompts at the next turn boundary instead of parking them'
status: open
priority: 1
issue_type: bug
assignee: mu-restore2@devbig030
labels:
- agentctl
created_at: 2026-11-03T02:01:04.799043542+00:00
updated_at: 2026-11-03T02:14:48.266117861+00:00
---

# Description

[opus 5.5] `agentctl send --queue-only` (f5292ccd) persists a prompt and returns with outcome pending, promising that "the next drain types it". Nothing drains it. On one host on 2026-11-02 every queue-only prompt a coordinator sent over about an hour was still pending: Muse lanes stay inside one turn for hours, and an idle Claude lane still held 3 queued prompts. One lane waited on a reply that never reached it. Until this is fixed, nobody may use --queue-only for anything that must be read promptly.

# Design

A queued prompt must be delivered at the target's next turn boundary, or by the watcher or a drain, and never just parked:
- An idle or done target: the next drain pass (the watcher, or a bridge or delivery loop that already runs) types it without waiting for another send.
- A working target whose harness accepts input at a tool boundary (Muse, Claude Code): deliver it there, as `send --urgent` already does, without interrupting the turn.
- The watcher reports queue-only prompts that stay pending past a threshold, naming the target and the age, instead of only "1 prompt waiting".

# Acceptance Criteria

- A queue-only prompt to an idle agent is typed within one watcher or drain interval with no further send.
- A queue-only prompt to a Muse or Claude Code agent mid-turn is taken at its next tool boundary without an interrupt.
- A test covers each case, in both editions.
- The --queue-only help says who delivers the prompt and when.
