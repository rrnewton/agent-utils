---
title: 'revive-keeps-queued-messages: a revive archives the messages queued to an agent instead of delivering them'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-12T11:38:25+00:00
updated_at: 2026-11-12T11:38:25+00:00
---

# Description

[opus 5.5] On 2026-11-12 the lead coordinator on one host was revived after a 15-hour outage (#1405 restore-epoch-manager-restart). The revive archived the messages queued to it during the outage without delivering them: about 40 messages, including other coordinators' requests. The lead had to read them back from the archive by hand. A revive is exactly when queued work matters most, and the sender already got a "queued" receipt, so archiving silently breaks at-least-once delivery.

# Design

- On revive (and restore, restart, turnover relaunch), queued prompts stay queued and are delivered once the new pane is ready, oldest first, through the normal queue (priority senders first).
- A queued item may be archived only when it can't be delivered on its own terms (expired, cancelled, its target retired). Each such archive records the reason, and `agentctl status` counts archived-undelivered items so the coordinator sees them.
- A prompt typed into the old pane whose outcome is unknown stays uncertain and is reported, not duplicated and not dropped.

# Acceptance Criteria

- A fixture test: queue three messages to a dead agent, revive it, and all three are delivered in order, with none archived.
- An expired item is archived with its reason, and status shows the count.
- The restore report lists per agent how many queued items were delivered and archived (feeds #1416 team-restore-verify).

# Notes

Numbered from devbig030's interim block (1400-1469). Part of the team-restore workstream; see the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).
