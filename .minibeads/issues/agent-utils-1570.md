---
title: 'scoped-coordinator-delegation: transferable scoped delegate for read-handoff, remove, recover'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] `read-handoff`, `remove`, and `recover`
require the coordinator PID in the invoking process ancestry
(`_assert_caller_process`). A delegated worker refused 48
read-handoff attempts and 1 remove with rc 3 in the 2026-11-15
drain, and a coordinator restart orphans every row leased to the old
generation. The lifecycle model (#1568) shows post-restart states
with no authorized actor at all.

# Design

Event-recorded scoped delegation: the recorded coordinator grants a
delegate generation for one slot (or a bounded batch), with expiry,
accepted beside ancestry by `_capture_caller_process` callers.
Revocation and every use are event-log entries. Direct ancestry
remains valid; nothing widens owner/liveness/salvage guards. Cover
coordinator restart: a new coordinator generation can re-lease rows
whose old coordinator generation is proven dead, recorded as an
event.

# Acceptance Criteria

- A non-descendant delegate with a live grant can read-handoff and
  remove the granted slot; an expired, revoked, or other-slot grant
  refuses and changes nothing.
- After coordinator restart, re-lease plus grant restores an
  authorized remover for a dead-owner slot (model DELEGATED flag).
- No grant path accepts batch attestation or skips occupancy,
  handoff, hold, or salvage checks.
