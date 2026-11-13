---
title: 'agentctl-queue-one-item-per-turn: deliver the whole queue at a turn boundary'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-13T11:24:51.871919871+00:00
updated_at: 2026-11-13T17:17:40.778051517+00:00
closed_at: 2026-11-13T17:17:40.778051407+00:00
---

# Description

agentctl delivers ONE queued item per turn boundary (headless Claude Code coordinator: ~35 short turns each started by one item; 67 queued, 30-90 min late; delayed not lost). Cross-host agent-teams relays also reached the peer queue 7-11 min after 'accepted', then surfaced one per turn over ~30 min, so peers acted on stale rulings.

# Design

At an idle turn boundary deliver ALL queued turn-boundary items as one harness input, each item keeping its own sender header, owner items first and individually headed (keeping the #698 intent: owner items stay visible and first, never silently folded). If the combined text would exceed the harness input limit, deliver in as few prompts as fit, owner items first, never truncating an item. Headless Claude wrapper first; TUI drain (Muse, Codex, Herdr panes) and agentcloud checked for the same one-per-turn pattern. Status reports the oldest queued age (all items); watch-agents alarms the manager when the oldest queued item passes an age cap. The relay-to-queue delay is traced (relay polling vs the peer serve/auto-drain path) and fixed here or split out with evidence.

# Acceptance Criteria

A queue of N turn-boundary items at an idle boundary is delivered in one turn (or as few size-bounded prompts as fit), owner first, no item truncated. Oldest queued age appears in status; the manager alarm fires past the cap in a test. Relay delay is fixed or a split issue with timing evidence is filed. Rust and Python parity where both editions have the path; differential cases pass.

# Notes

Evidence: headless coordinator session, ~35 short turns each started by one queue item, 67 queued 30-90 min late; cross-host relays accepted then queued 7-11 min later (five message IDs timed), surfacing one per turn over ~30 min. Earlier decision #698 (owner messages never batched) bounds the design.
Split: the cross-host accepted-to-queue delay (7-11 min) is #1490 agent-teams-relay-to-queue-delay; this issue covers the one-per-turn surfacing, status age and manager alarm.

## Resolution

Landed as dbb34919 with a5e35efd (handoffs and slash commands always go alone) and e06615ec (formatting). The headless wrapper delivers the whole turn-boundary queue as one input, owner first, size-split without truncation; status and the watcher expose and alarm on the oldest queued age. TUI drains and agentcloud showed no one-per-turn pattern; the relay delay is #1490.
