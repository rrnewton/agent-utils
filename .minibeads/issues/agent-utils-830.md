---
title: 'chat-bridge-warning-backoff: back off repeated delivery warnings and thread them'
status: open
priority: 1
issue_type: bug
assignee: cl-start@devbig030
labels:
- agentctl
created_at: 2026-11-03T22:00:33.296025718+00:00
updated_at: 2026-11-03T22:00:33.296025718+00:00
---

# Description

[opus 5.5] OWNER request (2026-11-03): the owner was sent "[chat-bridge WARNING] N of your messages to X have not reached it" every 5 minutes for over 4 hours while one coordinator's pane was orphaned. Repeated identical top-level warnings are noise that hides real alarms.

# Design

- Exponential backoff per stuck set (for example 5, 15, 30, 60 minutes, then every 3 hours). The backoff resets only when the stuck set or its reason changes, or after delivery recovers and then fails again.
- Repeats go as replies in the FIRST warning's thread, not new top-level posts, and update one rolling message where the provider allows.
- An explicit operator acknowledgement or snooze (`chat alarm ack|snooze`) that silences the warning while keeping the requests durable.
- Stay backward compatible with the existing ProviderDown and delivery-alarm records (#809 deliberately kept them strict). Leave room to switch the warning to a "replace message" operation later.
- After the fix: a feasibility note (harness ai_docs, with a changelog entry) on what the portable chat helper (Slack, Google Chat, Discord) and vibe-talk's message cache do today with message edits and deletes; a minimal "replace message" operation (delete and repost, or edit in place) with a per-provider capability flag; how vibe-talk's cache would record a deletion as an append-only tombstone; which message types should use replace (alarms, status cards) while everything else stays append-only.

# Acceptance Criteria

- With a fake clock, 4 hours stuck produces at most about 7 posts, only 1 of them top-level.
- A change of stuck set or reason resets the backoff; recovery then failure resets it.
- ack and snooze work and keep the requests durable; both are documented in help.
- The feasibility note is linked here.
