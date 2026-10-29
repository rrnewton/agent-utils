---
title: 'chat-bridge-reminder-harness-name: name the detected harness in reply reminders'
status: open
priority: 2
issue_type: bug
created_at: 2026-10-29T12:31:44.244114949+00:00
updated_at: 2026-10-29T12:31:44.244114949+00:00
---

# Description

[watermelon-full-preview] Owner request 2026-10-29 (chat-bridge-robustness batch).

Reply reminders mislabel the harness: the reminder path only ever asked a Claude Code CLI whether the coordinator is idle, so a coordinator running in a Muse (or Codex) pane was never reminded, and the bridge's startup and wait text said "Claude Code" whatever harness the pane actually ran.

Wanted: use the detected harness everywhere. A known non-Claude harness answers idleness from its pane status; the transcript leave-out stays Claude-specific; every reminder log line names the harness detected in the pane.
