---
title: 'restart-when-idle: restart a live agent in its own pane with its recorded session'
status: open
priority: 0
issue_type: feature
created_at: 2026-11-06T21:04:18.359346800+00:00
updated_at: 2026-11-06T23:38:32.087131861+00:00
---

# Description

R1 (owner reply 174): agentctl restart NAME --when-idle waits for an idle turn with nothing queued, exits the harness gracefully (Claude /exit with background-dialog abort; Muse /exit then Ctrl-C twice), relaunches in the same pane/tab resuming the recorded session, re-pins, restores the Herdr name, transfers wrkslots slot ownership, and queues a re-arm notice. Refuses cleanly when the session is unknown/contradictory. Lessons in ai_docs/transient/2026-11-06-restore-in-place/LESSONS.md.
