---
title: 'watch-notice-backoff: the undelivered-owner-chat notice backs off, is timestamped, and replaces its pending copy'
status: closed
priority: 0
issue_type: bug
assignee: cl-start
created_at: 2026-11-04T15:21:36.245953428+00:00
updated_at: 2026-11-04T15:27:16+00:00
closed_at: 2026-11-04T15:27:16+00:00
---

# Description

The owner (2026-11-04, via the lead): watch-agents' notice to a coordinator, "the owner's chat messages to NAME are not being delivered ... This notice repeats every 5 min until they are delivered", has no backoff and no timestamp, and the copies piled into the coordinator's pane: "Not timestamped either and every 5 min without backoff is crazy".

# Design

- Apply #830's warning backoff to this notice: repeat at 1, 3, 6 and 12 intervals, then every 36; reset when the reason changes.
- Each notice carries the time (ET) and the age of the undelivered owner message(s).
- Never queue a repeat while the previous copy is still undelivered in the coordinator's queue: replace it instead.

# Acceptance Criteria

- Tests with a fake clock cover the backoff schedule, the reset on a changed reason, the timestamp/age text, and replace-not-append while a copy is pending.
- Gate passes; landed alone (P0).

# Notes

P0 by owner direction; assigned to the #830 author (cl-start), ahead of #839.

## Resolution

[opus 5.5] The watch-agents owner-stuck notice now:

- **Backs off** on #830's schedule. With the default `--owner-stuck-minutes 5`, notices go out at 5, 10, 25, 55 and 115 min, then every 3 h, never every 5 min. The backoff restarts one interval after the last notice when the kind of reason changes (the chat bridge's `owner_alarm::reason_kind`).
- **Opens with the time it was written** ("[agentctl watch-agents] 19:05 ET: …") and says since when the oldest held request has waited ("the oldest for 5 min (since 19:00 ET)").
- **Replaces, never appends.** A repeat first withdraws the previous copy from the coordinator's queue (`cancel_message` tombstone: an unread copy is set aside, never typed), so at most one waits there.

The coordinator's own queue keeps its once-per-episode notice. The state file gains `owner_stuck_backoff` per agent (kind, step, last/next, message ID) and still writes `owner_stuck_reported` for older builds; a state from an older build carries its last notice time into the backoff.

Tests (fake clock):
- `owner_stuck_notices_back_off_reset_on_a_new_reason_and_replace_the_unread_copy`: a 4 h schedule, the ET time and age text, replacement of every unread copy, and a reset on a changed reason.
- `a_state_from_an_older_watcher_carries_its_last_notice_into_the_backoff`.
- The existing owner-request test, updated.
