---
title: 'unknown-tui-bridge: unrecognised TUIs fall back to the unknown harness, agentterm is recognised, and the bridge captures their replies'
status: open
priority: 1
issue_type: feature
assignee: cl-start
created_at: 2026-11-04T22:42:10.018251750+00:00
updated_at: 2026-11-05T01:30:00.000000000+00:00
---

# Description

The owner (2026-11-04) wants a chat bridge attached to a coordinator that runs in an agentterm tab: an agentcloud session viewed through agentterm. Today:
- adopt, start and bridge targets refuse an unrecognised TUI unless `--harness unknown` is given explicitly (#850);
- an agentterm pane already registered as an agentcloud record cannot be a bridge target ("Herdr-hosted harnesses only");
- for an unknown-harness target, the bridge has no way to capture replies: its reply text comes from a Claude/Codex/Muse transcript, which an agentterm pane does not have.

The workaround used on 2026-11-04 was a second agentterm view of the same session, adopted as `--harness unknown`, with replies only through `agentctl chat reply N --file`.

# Design

- Fallback: when a pane's TUI is not recognised, adopt, start and the bridge's target checks fall back to the unknown-harness mode automatically, with a clear note, instead of refusing. An explicit `--harness KIND` mismatch is still refused.
- agentterm as a recognised harness, or at least a clean unknown-harness profile for it: idle/working detection from its status line ("idle", "working", "send a message to ..."), composer location, steer vs send, and its quit key.
- Bridge reply capture for unknown/agentterm targets:
  - from screen or scrollback: CHAT_REPLY blocks, or the agentterm transcript view if one exists;
  - or `chat reply --file` only, stated in the request prompt the bridge types.
  - The bridge must never fall back to another agent's transcript (today the health readout kept the previous agent's transcript as the reply source after a repoint).
- A pane registered as an agentcloud record can be targeted through its agentterm view, without a duplicate record.
- Out of scope (owned by the consumer team's teams-cloud-fix lane): the agentcloud session path itself, and agent-teams inbound delivery to cloud sessions.

# Acceptance Criteria

- Tests with recorded agentterm screens: idle/working detection, typing into the composer, and refusal on a stale pane.
- A bridge repointed at an agentterm target types requests and captures replies by the chosen method. After a repoint, health names the right reply source and never the previous agent's transcript.
- Unrecognised-TUI fallback tests for adopt, start and repoint.
- Userguide/help updated; gate passes.

# Notes

P1, owner via the lead (2026-11-04 17:36-17:41 ET). Assigned to cl-start after #848.

## Progress

[opus 5.5] cl-start, 2026-11-04 (P0 landing, owner re-order 17:55 ET: make agentterm work as an unknown TUI first).

- The transcript reader follows the current target: a repoint retargets it (`BridgeState::serving`), dropping what it read of the previous agent, so the new agent's screen replies are no longer held back as unpaired (the 60 s delay and refused tables after a repoint). It rests for a headless target, and is built even when `chat run` starts on one. For a target whose harness writes no transcript agentctl reads (`--harness unknown`), the reply source says the screen. `chat health` never shows a `reply-source.json` recorded for another agent.
- Receipts follow the target too: the receipt mode, prompts that offer only the command, and the latency profile all switch with the target kind. Tested.
- Screen capture for margined TUIs (`chat_runtime/screen_layout.rs`). The silent loss happened because agentterm draws every row one column in, and the scanner opens no block above the first row at column 0. Now the margin, the bottom input frame and the right side panel are removed, `You:` rows read as prompt rows and `Agent:` hanging indents are removed. Tested with screens laid out like the recorded agentterm screens, and checked against a real recorded coord7 screen, which yields block 047 whole.
- Lenient tags on those screens: `Agent: <CHAT_REPLY_N>`, text after an opening tag or before a closing tag is body, text after a closing tag is dropped, and a single-row block works. Word-wrapped rows are joined.
- On every harness, the bridge's own prompt rows (the `reply: <CHAT_REPLY_N>` header field and the `Reply between a line ...` instruction) are never read as tags. Every routing notice ends with `agentctl chat reply ID --file FILE`.
- Remaining: (P0b) lenient tags on every harness, plus errors for mangled tags (this reverses the inline-tag policy and its 34 tests); agentterm recognition and transcript harvesting; the auto-fallback to unknown (B); an agentcloud record through its agentterm view.
