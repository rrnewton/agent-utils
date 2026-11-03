---
title: 'agentctl-composer-unreadable-escalation: find why a working composer was unreadable, and escalate long refusals'
status: closed
priority: 1
issue_type: bug
assignee: cl-start@devbig030
labels:
- agentctl
- agent-teams
created_at: 2026-11-03T22:03:33.202983463+00:00
updated_at: 2026-11-03T23:13:20+00:00
closed_at: 2026-11-03T23:13:20+00:00
---

# Description

[opus 5.5] A third stranding mode, after #808 and #825: for about an hour on 2026-11-03 (16:00-17:00 ET) agent-teams auto-drain could not deliver eleven messages to a working Claude Code coordinator; each attempt refused with "composer_unreadable: pane ... does not show a recognisable claude composer after 15.0s; ansi_blank=true scroll_offset_from_bottom=0 working=true interactive_ready=true foreground_match=true; nothing was typed" (from the auto-drain stderr log #808 added). The refusals failed silently: neither the sender nor the watcher was told, and an owner P0 waited an hour.

# Design

- Find why the composer was not recognised while the pane was working and in the foreground (ansi_blank=true suggests the capture read a blank or redrawing composer region), and fix the recognition or the capture.
- A long refusal escalates: after N minutes of refusals for the same target, alarm the sender (agent-teams receipt) and the watcher (a finding naming the target, the reason and the age) instead of failing silently.

# Acceptance Criteria

- The cause of the unrecognised composer is written in the issue, with a regression test from a captured screen.
- A test with a composer that stays unreadable produces a watcher finding and a sender-visible receipt after N minutes.

# Notes

## Cause of the unreadable composer (found 2026-11-03)

[opus 5.5] The composer was unreadable because the pane's terminal grid really was empty, as Herdr holds it. agentctl's recognition was not at fault: the screen it captured (ansi_blank=true) is what Herdr serves for that pane. Read-only evidence on the affected coordinator:

- Every Herdr read source (`visible`, `recent`, `recent-unwrapped`, `detection`) returned no text, and `scroll.max_offset_from_bottom` was 0 (no scrollback at all). The other Claude Code panes in the same workspace rendered normally, with 1,000+ rows of scrollback.
- The harness was alive: it was the pane's foreground process, its OSC title spinner kept changing, and it answered messages it read from its queue by hand.
- Its transcript shows it idle from 20:59:32 to 21:58:00 UTC, the whole refusal hour, while Herdr's title-derived status said `working`. A background task notification, not a prompt, woke it.
- With the coordinator's permission, one Ctrl+L (Claude Code's clear-and-redraw) was sent. The grid stayed empty and the title kept updating, so a redraw key does not recover this state. "Send a redraw key, then probe again" was therefore not built.
- Context: that Claude Code process had run for 30 h, its binary had since been replaced on disk, its transcript was 272 MB, and its native process made about 88k small writes per second (a fresh session made about 260). Why it stopped drawing is not established. A restart of the harness on the same session is the expected fix; it was not run, pending the owner.

The captured refusal string is the regression fixture in the watcher tests (`UNREADABLE`).

## Delivered

- **Watcher escalation** (`agentctl watch-agents`). Every pass already drained each queue and marked a remainder past `--queue-stale-minutes` (default 10) stale, but told no one. A stale queue whose drain was refused without typing (composer_unreadable, a refused recipient check, a draft) is now escalated once per hold, as a priority sender. A new kind of refusal is a new hold; delivery ends the hold.
  - For an agent's queue, the coordinator is told.
  - For the coordinator's own queue, where that notice would wait too, each local agent with a message held there is told, from the queue items' sender headers.
  - agent-teams senders are named in the pass report's `held` rows.
  - A busy agent is never escalated.
- **Sender-visible receipt** (agent-teams). The auto-drain now reads `agentctl drain`'s JSON report (and still appends it to `auto-drain-AGENT.stderr.log`). When it typed nothing and gave a `blocked` reason, the ledger entry stays `accepted` but gains `held` {since, reason, checked_at}, and its `detail` begins "held at the receiver since". `receipt` returns both, so the sending host's receipt walk sees why. A terminal state removes `held`.
- Regression tests: `the_captured_blank_coordinator_screen_is_classified_as_the_incident_recorded_it` (submission), the two held-queue watcher tests, and the held receipt in agent-teams' `reconcile_folds_queue_terminal_state_into_the_ledger`.
- Not done: the blank-working health check still skips the coordinator. The held-queue escalation covers the coordinator's case and tells the people who can act.
