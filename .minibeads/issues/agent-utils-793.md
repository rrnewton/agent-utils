---
title: 'chat-bridge-relay-refeed: the bridge re-dispatches the same relay post to relay-in hundreds of times'
status: closed
priority: 2
issue_type: bug
assignee: mu-mirror@devbig014
labels:
- agentctl
created_at: 2026-11-02T21:43:59.683284815+00:00
updated_at: 2026-11-02T23:31:52.605419440+00:00
closed_at: 2026-11-02T23:31:52.605419340+00:00
---

# Description

[opus 5.5] 2026-11-02 on one host: the chat bridge fed the SAME relay post (an unparseable-post receipt; quarantine id unparseable-278e612e9310) into 'agent-teams relay-in' about 658 times in 87 minutes, although dispatch_relay_post dedupes by message id in relay-dispatched.json. Find what re-feeds it (the rescan path re-offering a thread reply? the live copy and the rescan copy differing, e.g. by an appended rescan note, so the dedupe key differs? thread replies keyed differently from roots?), and make relay dispatch at-most-once per provider message id across live and rescan paths, with a test. The receiver now records a post at most once (#763 rework), but the bridge must not re-dispatch.

# Notes

[opus 5.5] Lead 2026-11-02: assigned to mu-mirror@devbig014; order #763, #793, #769, #790 (#769 reworks the same bridge relay dispatch path, so one lane owns both).
