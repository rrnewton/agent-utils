---
title: 'vt-voice-stream-leak: private voice stack leaks Oxide streams/push subs, nightly 101 without session_started'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-31T07:21:06.830049409+00:00
updated_at: 2026-10-31T07:21:06.830049409+00:00
---

# Description

On the private voice stack (Oxide + voice bridge + loopback services) the voice path fails nightly with "101 but no session_started", about 21-23 h after each Oxide+bridge restart.

Ops evidence (2026-10-31, hourly STATUS series since 09-22):
- Oxide STATUS streams/push_subs climb while sessions=0 (7/5 at 30 min -> 167/56 at 22h51m).
- At 10 h uptime the count was <=8 through 10-14, 42 on 10-22, 95 on 10-28, so the leak began between 10-14 and 10-22.
- Bridge failure signature: "persistent stream reopen failed" then "failed to build oxide client ... Resource temporarily unavailable (os error 11)".

A stopgap timer restarts Oxide+bridge daily at 04:00 PT.

Task: find who opens the streams/push subscriptions that are never released (vibe-talk relay/conversation backend, read-aloud, smoke probes, watchdog probes, or the bridge itself). Bisect by date against agent-utils main commits touching vibe-talk voice/relay/read-aloud code and the deploy history. If the leak is in vibe-talk server code, fix it with a regression test; if in probes, propose the fix; if in bridge/Oxide, write an evidence-backed report with a minimal reproduction.

Report: ai_docs/2026-10-31-voice-stream-leak.md
