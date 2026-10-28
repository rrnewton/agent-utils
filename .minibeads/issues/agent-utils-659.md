---
title: 'agentctl-resume-archived: resume a finished agent from its archive, with wrkslots restoring its slot'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
- wrkslots
created_at: 2026-10-28T15:37:24.118273727+00:00
updated_at: 2026-10-28T15:37:24.118273727+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 11:19-11:30 ET: finished agents are closed but must stay resumable. Archives keep the session id (e.g. archive/cl-vtside-*/agent.json --session-id), and 'agentctl start --resume ID' exists, but: no 'agentctl resume <archived-name>'; Claude Code (and others) key sessions by project directory, so after slot removal the session cannot be resumed from a new cwd. Build: wrkslots restore (recreate a slot from an archived one's branch or refs/rescue snapshot under a new name); agentctl resume (look up id, harness and profile; restore the slot; map the old session, e.g. link the transcript under the new directory key; --resume); list resumable archived agents; record the transcript path in the archive and the wrkslots handoff.
