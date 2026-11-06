---
title: 'unknown-tui-bridge: unrecognised TUIs fall back to the unknown harness, agentterm is recognised, and the bridge captures their replies'
status: closed
priority: 1
issue_type: feature
assignee: cl-start
created_at: 2026-11-04T22:42:10.018251750+00:00
updated_at: 2026-11-05T14:36:49.000000000+00:00
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
- D (the owner's requirement, not an extra): unknown-pane receipts require evidence; otherwise the request is uncertain. A delivery to an unknown-harness or agentterm pane gets a delivered receipt only with positive evidence that the input reached the agent (for an agentterm view of an agentcloud session, the session's agentcloudctl history showing the input). Without evidence it is uncertain, never retyped blindly, and alarmed per the #896 policy. Owner's words: "Unknown should still never justify false-positive claim of delivery to the agent when nothing is delivered."
- D: an agentterm pane attached to an agentcloud session captures replies from that session's agentcloudctl history (moved here from #896 on 2026-11-05).
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
- P0b (cl-start, 2026-11-04): lenient tags on every harness and in an agentcloud session's history text, which goes through the same scan. To keep prose quotes from being read as tags, an opening tag counts only at the start of a message, a paragraph or a row after a finished sentence, or after a lead that ends with a colon, and not inside an open block. A closing tag counts where it closes the open block of its ID, or ends a finished sentence at the end of its row when no block is open. Tags elsewhere (mid-sentence, code spans, fences, tool calls) keep the inline-tag report. Tags of an open request written with a character missing or extra (`<CHAT_REPLY_N` with no `>`, `< CHAT_REPLY_N >`, `&lt;...&gt;`, `[CHAT_REPLY_N]`, a lowercase name) are reported once as `mangled`, with a notice naming `agentctl chat reply ID --file FILE`. Known gap: `CHAT_REPLY_N>` is not reported, because that is how the bridge's own `‹` quotation reads once a pane drops the `‹`. The 34 inline-policy tests were rewritten: tags at a message start now send, and the inline-report tests use a prose lead.
- Remaining: agentterm recognition and transcript harvesting (C); the auto-fallback to unknown (B); an agentcloud record through its agentterm view (D).
- C (cl-start, 2026-11-05): an agentterm pane driven as `--harness unknown` is read by its input frame (`crate::harness::agentterm`).
  - `╭ idle ─…` is idle, and a spinner with `working M:SS` is working. Before, a long silent turn looked idle after 2 s, and the ticking idle clock looked busy. Any other label reads `unknown`.
  - A delivery is refused during a turn (Enter would steer it) and under another label.
  - A composer draft that is not a placeholder is refused with the shared `composer already holds unsubmitted text` wording, so the bridge's alarm and draft reporting see it. An earlier pass's own stranded paste still gets Enter.
  - Tested with screens laid out like the recorded idle and working agentterm screens (names replaced), and through the fake TUI.
  - Not done: the stale-composer clear for an agentterm draft (agentterm's clearing keys are unverified), and transcript harvesting. Replies are captured from the screen, as done in P0, or with `chat reply --file`.
- D (cl-start, 2026-11-05): unknown-pane receipts need evidence, and an agentterm view of an agentcloud session uses that session's history.
  - A pane driven by the generic rules gives a receipt only when the screen shows the prompt submitted: our text above the bottom rows, or on agentterm a new `You:` turn in the conversation that holds the prompt's first 64 letters and digits (`harness::agentterm::submitted_turns`, with a side panel cut off first).
  - Text that only left the bottom, a screen that only changed (taken before Enter, a paste never drawn), or the text in the composer or agentterm's `↳ queued:` row proves nothing. After 30 s with no evidence the outcome is unknown: the queue keeps the prompt as possibly submitted, never types it again, and the bridge reports the request uncertain and alarms on it.
  - A pane whose process runs `agentterm -s SESSION` (found through the process whose environment names the pane; `--ws-url` or its `AGENTCLOUD_ORCHESTRATOR_URL` give the endpoint) takes its evidence from that session's history. A prompt the screen could not prove counts as `Processed` once a `user_input` holds its opening, even behind terminal colour replies (#900), and that is recorded in `cloud-inputs/`.
  - Its replies are captured from the session's history, and the transcript reader names that history as the reply source, as for an agentcloud record. Prompts to it stay command-only (#896).
  - Tests:
    - recorded-layout `You:` turns, the composer and the queued row;
    - the fake TUI with a swallowed input, an agentterm view with no turn, and a prompt stranded in the composer;
    - process discovery, and history matching by text;
    - an adopted unknown pane viewing a session: uncertain, then delivered from history, then replies captured from history.
- B (cl-start, 2026-11-05):
  - `adopt --harness NAME` for a NAME agentctl has no adapter for (such as `agentterm`), in a pane where Herdr reports no agent, adopts the program with `--harness unknown` and says so in the result's `note`, instead of refusing. A harness agentctl knows is never swapped: a pane where Herdr does not see it is still refused.
  - `chat repoint` to an agent in whose pane Herdr detects no harness now names the way out: `agentctl adopt NAME --pane ID --harness unknown`, then repoint to that name. An unknown-harness agent is already a valid repoint target, and since #896 its prompts are command-only.
  - `start` still does not launch an unknown TUI, and points at `adopt`: there is no launch command to run for a program agentctl has no adapter for.
  - Reply reminders for a pane driven by the generic rules read agentterm's input frame: `idle` is idle; `working` or any other label is busy. Before, such a pane was never reminded ("unrecognized harness"). Any other unknown TUI is still not reminded. The frame is read through the manager, so the pane-info struct is unchanged.
  - Tests: the adopt fallback with its note, a known harness still refused, and reminders by the frame (idle, working, another label, no frame).

## Resolution

[opus 5.5] cl-start, 2026-11-05. Delivered in parts:
- P0 and P0b: tags, prompt rows, routing notices.
- C (`b55a437b`): agentterm read by its input frame.
- D (`5b0a0f6c`): unknown-pane receipts need evidence; an agentterm view of an agentcloud session uses that session's history for evidence and replies.
- B (this change): the adopt fallback, the repoint refusal text, reminders by the frame.

Not done:
- **The stale-composer clear for an agentterm draft.** agentterm's clearing keys are unverified, so a draft is refused, never cleared.
- **The colour-reply noise in agentterm pastes.** That is #900.
- **A consumer-side live check of D** goes through the lead.

D scope set by the lead on 2026-11-05 at 1:04 PM ET: #896 left unknown-pane receipts unchanged to keep an interim agentterm target working, so evidence for unknown panes and agentterm-on-cloud history capture land here. Queued right after #896 and #897.
