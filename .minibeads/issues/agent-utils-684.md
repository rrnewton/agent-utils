---
title: 'chat-bridge-reminder-harness-name: name the detected harness in reply reminders'
status: closed
priority: 2
issue_type: bug
created_at: 2026-10-29T12:31:44.244114949+00:00
updated_at: 2026-10-29T16:24:52.228738369+00:00
closed_at: 2026-10-29T16:24:52.228738259+00:00
---

# Description

[watermelon-full-preview] Owner request 2026-10-29 (chat-bridge-robustness batch).

Reply reminders mislabel the harness: the reminder path only ever asked a Claude Code CLI whether the coordinator is idle, so a coordinator running in a Muse (or Codex) pane was never reminded, and the bridge's startup and wait text said "Claude Code" whatever harness the pane actually ran.

Wanted: use the detected harness everywhere. A known non-Claude harness answers idleness from its pane status; the transcript leave-out stays Claude-specific; every reminder log line names the harness detected in the pane.


Closed: landed on main as 525b06fe (gate rc=0, validate: OK; agentctl Rust tests 1745 passed standalone in 420 s). Unverified: no live bridge round-trip by this lane; coordinator deploys.
