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

# Notes

## Delivered in the first commit

[opus 5.5]
- **Backoff.** The owner alarm backs off per episode: one top-level warning, then each repeat (opening "Still:") as a reply in its thread. Repeats come after 1, 3, 6 and 12 times `--owner-alarm-after`, then every 36 times it: 5, 15, 30 and 60 minutes, then every 3 hours, by default.
- **Resets.** An episode's fingerprint is the kind of reason plus the held request keys. The kinds are foreign draft, unsent draft, refused input, status disagreement, missing pane, and other. The exact words of the reason are not used, since they change with every attempt. A change of fingerprint restarts the backoff in the same thread. Recovery is announced in the thread, and a later failure starts a new top-level warning.
- **Restarts.** The episode, including its first warning's message and thread, is saved in `owner-alarm.json` (schema agentctl-chat-owner-alarm/v1), so a restarted bridge carries on in the same thread.
- **Threads.** `RootMessageTransport::publish_in_thread` is new; the helper's send path implements it. A provider that refuses a thread gets the repeat at the top level, which then starts the thread.
- **ack and snooze.** `agentctl chat alarm [status|ack|snooze DURATION|clear] [--json]` reads and writes `owner-alarm-silence.json`. ack silences until the fingerprint changes; snooze silences until a time. Neither touches a request.
- **Compatibility.** `delivery-alarm.json`, `ProviderDown` and the other #809 records are unchanged.
- **Later "replace message".** The saved first-warning message ID leaves room to rewrite it in place.
- **Tests:**
  - `four_hours_stuck_post_few_warnings_and_one_at_the_top_level`: 5 posts in 4 hours, 1 top-level.
  - The reset test: set change, reason change, recovery then failure.
  - The ack/snooze test.
  - The service test `the_owner_alarm_repeats_in_its_thread_across_a_restart_until_acknowledged`: thread, restart, ack keeping the request pending, recovery in the thread.
- **Docs:** the chat user guide, and the `chat run --owner-alarm-after` and `chat alarm` help.

Remaining: the feasibility note on a "replace message" operation (last bullet of the Design), to be linked here.
