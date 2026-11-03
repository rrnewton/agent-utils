---
title: 'chat-dead-pin-reanchor: re-anchor or name the fix when a pinned harness died and a live agent holds the pane'
status: open
priority: 1
issue_type: bug
assignee: cl-start@devbig030
labels:
- agentctl
created_at: 2026-11-03T22:00:33.302217208+00:00
updated_at: 2026-11-03T22:00:33.302217208+00:00
---

# Description

[opus 5.5] Root cause of the 2026-11-03 warning spam (#830): a coordinator was restarted, but the bridge's registry record still pinned the dead launcher process, so every pass refused input ("refusing input to agent ...: the anchor ...") while the warning only said "not reached it". `agentctl anchor --replace` fixed it at once.

# Design

When a pinned harness process is dead but the same pane now runs a live agent of the same kind, either re-anchor automatically (with a journal line saying so), or at least make the warning name the reason and the exact fix command. Never loop silently.

# Acceptance Criteria

- A test with a dead pin and a live same-kind agent in the pane yields either delivery (after an automatic re-anchor, journaled) or a warning that names the reason and the fix command.
