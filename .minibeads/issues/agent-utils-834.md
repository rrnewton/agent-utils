---
title: 'agentctl-composer-unreadable-escalation: find why a working composer was unreadable, and escalate long refusals'
status: open
priority: 1
issue_type: bug
assignee: cl-start@devbig030
labels:
- agentctl
- agent-teams
created_at: 2026-11-03T22:03:33.202983463+00:00
updated_at: 2026-11-03T22:03:33.202983463+00:00
---

# Description

[opus 5.5] A third stranding mode, after #808 and #825: for about an hour on 2026-11-03 (16:00-17:00 ET) agent-teams auto-drain could not deliver eleven messages to a working Claude Code coordinator; each attempt refused with "composer_unreadable: pane ... does not show a recognisable claude composer after 15.0s; ansi_blank=true scroll_offset_from_bottom=0 working=true interactive_ready=true foreground_match=true; nothing was typed" (from the auto-drain stderr log #808 added). The refusals failed silently: neither the sender nor the watcher was told, and an owner P0 waited an hour.

# Design

- Find why the composer was not recognised while the pane was working and in the foreground (ansi_blank=true suggests the capture read a blank or redrawing composer region), and fix the recognition or the capture.
- A long refusal escalates: after N minutes of refusals for the same target, alarm the sender (agent-teams receipt) and the watcher (a finding naming the target, the reason and the age) instead of failing silently.

# Acceptance Criteria

- The cause of the unrecognised composer is written in the issue, with a regression test from a captured screen.
- A test with a composer that stays unreadable produces a watcher finding and a sender-visible receipt after N minutes.
