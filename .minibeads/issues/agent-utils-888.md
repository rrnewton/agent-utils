---
title: 'chat-alarm-settled-stall: the delivery alarm drops requests that are already settled'
status: open
priority: 3
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T14:20:39.962862275+00:00
updated_at: 2026-11-05T14:20:39.962862275+00:00
---

# Description

The chat bridge's delivery alarm keeps settled requests in its `stalled` list.

Seen on 2026-11-05 on a consumer host, agentctl d8447c04:
- A request from 2026-10-30 ended in phase `delivery_uncertain` ("submitted but never showed in pane … within 5s").
- It was settled on 2026-11-03 with `chat settle --settle-as abandoned`. Its record carries the settlement (as abandoned, by the operator, reason "superseded by a later message"), and a second `chat settle` correctly answers "already settled; a settlement is final".
- `chat health` counts it correctly (0 stalled, 0 uncertain), and `chat alarm status` reports nothing held.
- But the running bridge still rewrites `delivery-alarm.json` with that request under `stalled`, with its original reason. It did so again right after a restart at 09:16 ET.

No supported command removes the entry; `chat alarm clear` only ends an episode. No warning repeats, so the impact is low. But the file looks like a live stall, and its readers (the watcher's owner alarm, operators) are misled.

# Design

- The alarm's stalled/uncertain computation uses the same predicate as `chat health`: a request with a recorded settlement (delivered or abandoned) is excluded.
- Rewriting `delivery-alarm.json` drops entries whose requests are settled, including entries that an earlier version carried over.
- Ideally `chat health`, `chat alarm status` and the alarm file share one function for "stalled".

# Acceptance Criteria

- A test settles a `delivery_uncertain` request as abandoned, then as delivered. After the next bridge pass, or a restart, `delivery-alarm.json` no longer lists it, and health and alarm status agree.
- An alarm file that already lists a settled request is cleaned on the next rewrite.

# Notes

Reported by the lead on 2026-11-05 at 09:20 AM ET (P3). The reporting host left the file untouched for reproduction. Assigned to cl-start after #887.
