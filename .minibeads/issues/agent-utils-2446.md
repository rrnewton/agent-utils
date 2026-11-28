---
title: 'team-queue-late-tag: a team message that goes late or is superseded while waiting in an agentctl queue is typed with its warning field'
status: open
priority: 1
issue_type: bug
assignee: cl-bridgeretire
labels:
- agent-teams
- agentctl
created_at: 2026-11-28T01:41:00-05:00
updated_at: 2026-11-28T01:41:00-05:00
---

# Description

A follow-up to #2420 relay-never-lossy, step (c) gate 1 (R4b and R7). When agent-teams delivers a late or superseded message, it puts the warning on line 1, as a `warning:` field of the team tag: `late: its not_after T passed 3h ago; reconfirm before acting`, or `superseded by <id>: reconfirm before acting`. The point is that a late post never reads as a fresh instruction.

That works for a message that is already late or superseded when it arrives, and for one that goes late or is superseded while it waits in a hook inbox, because the hook drain adds the field when it prints. It does not work for a message waiting in an agentctl queue:
- Team items in the `inbox.v2` lane carry no `not_after` since #2420 R4b. That stops the drainer from setting a late item aside untyped, which was a silent loss. But a message that goes late while queued is now typed with no late field; only its meta line still shows `expires T`.
- A message a newer one supersedes after it was queued is typed as queued, with no `superseded by` field, because #2420 R7 no longer writes a tombstone and the item's text cannot change after enqueue.

# Design

- At enqueue, put the late deadline on the team item as a field the drainer reads, for example `late_after` (Unix ms). Do not use `not_after`, so older drainers still type the item instead of setting it aside.
- In agentctl's drainer, just before typing an inbox.v2 item that carries `late_after` in the past, add the warning field to its first line, the sender header (agent-teams' `add_header_warning` rule: append ` · warning: ...` inside the bracket). Every reader of line 1 still parses a valid header: the stale-draft attribution, the paste backstop, the failed digest and the headless wrapper.
- For a superseded item, the drainer reads `superseded_by` from the item's external delivery record (the agent-teams ledger entry it already names) and adds `superseded by <id>: reconfirm before acting` the same way.
- The text compared after typing (the drainer checks that the typed text equals the item's text) must use the text actually typed.

# Acceptance Criteria

- A test enqueues a team item whose `late_after` passes before the drain. The typed prompt's first line is a valid sender header carrying `· warning: late: ...`, and the item is typed, not set aside. It fails on main.
- A test enqueues an item, then marks its ledger entry `superseded_by`. The typed prompt's first line carries `superseded by <id>: reconfirm before acting`. It fails on main.
- An older drainer (an item without `late_after`) types exactly as today.

# Notes

- Numbered #2446 by coordb (2026-11-28). It is the known limit recorded in the #2420 step (c) gate 1 land note.
