---
title: 'harness-agnostic-stuck-paste-backstop: one general watcher backstop submits a stuck delivered prompt in any TUI'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-15T12:30:00.000000000+00:00
updated_at: 2026-11-15T12:30:00.000000000+00:00
---

# Description

Owner 082 (2026-11-15), following owner 069 (2026-11-14): prompts agentctl
delivered sat pasted and unsubmitted in TUI input boxes; the deterministic
backstop that should spam Enter never fired. #1546
agentctl-unsubmitted-paste-backstop (landed 4cbc07e4) fixed the per-harness
composer readers, but the watcher sweep (`sweep_editor_drafts` in
`rs/agentctl/src/agent_watch.rs`, #731) still gates on a parseable composer:
`editor_draft` returns no draft when no harness reader recognises the screen,
so an unknown TUI, and any busy layout a reader cannot split, gets at most a
`draft: unreadable` notice and never an Enter. The owner asked for one
general heuristic instead: remember what agentctl sent, fuzzy-match the
pane's lower region against it, and press Enter when it stays stuck — for
Claude, Codex, Muse and unknown TUIs alike, with the per-harness composer
readers kept only as optional refinements.

Related: #1546 (per-harness sweep and `draft: unreadable`), #1553
codex-footer-composer (its swallowed-Enter symptom points here), #705
oversized-prompt (long pastes collapse to placeholders), #556
chat-delivery-latency, #681 chat-bridge-reply-permanent-failure, #671
headless-approval-surface (the approval path Enter must never answer).

# Design

One harness-agnostic backstop in the watcher, beside the existing sweep:

- Memory: per agent, the prompts agentctl delivered, read from the agent's
  queue records (`processed/`, `failed/`, stranded `inbox`/`inbox.v2`), each
  with its queue ID and typed time. Only framed agentctl deliveries count.
- Match: the pane's last 24 visible rows, screen text only. Normalise by
  stripping box-drawing and prompt glyphs and removing all whitespace. A
  delivered prompt matches when its whole normalised text appears in the
  region (short prompts), or its first and last 40 normalised characters
  both appear (long prompts), so wrapped or re-flowed text still matches.
- Trigger: the same prompt matches on 3 consecutive polls spanning at least
  45 s, and is still matched on the pressing poll. At most 3 Enter presses
  per prompt, at least 20 s apart. Every press is logged, reported in the
  pass report, and noticed to the coordinator; still stuck after the cap, a
  BLOCKED notice names the pane. A pane is never silently skipped: every
  poll emits a backstop row.
- Consumed: the text leaves the lower region, or a turn starts or ends; the
  remembered prompt is then cleared. Entries expire after 2 h.
- Safety: Enter is pressed only for a prompt matched against agentctl's own
  delivery records. No press while an approval, permission or picker prompt
  shows (the harness-agnostic `approval_prompt_line` union table, the peer
  approval dialog, and the known-harness question/picker classification);
  when unsure, the backstop notices and does not press.
- The per-harness sweep runs first on the same screens and remains the
  faster, placeholder-aware path; when it flushes an agent on a pass, the
  general backstop records that pass but does not also press. A reader that
  cannot parse never gates the general path. Enter goes through a new
  bare-Enter notifier action that takes the lifecycle lock and checks input
  is allowed, with no composer verification.
- Python edition: `watch-agents` is Rust-only there, so there is no Python
  watcher to mirror; a parity note records this.
- The harness support matrix (#1561) gains the backstop row in the same
  commit, or its row values go to that lane if it has not landed.

Defaults live in one config struct so tests drive poll counts and fake
times, never wall-clock budgets.

# Acceptance Criteria

- Fake-pane fixtures for Claude, Codex, Muse and an unknown TUI shape: a
  stuck delivered paste leads to Enter and is then consumed.
- A normal submit (text gone next poll) means no press.
- Text agentctl did not deliver means no press.
- An approval or permission prompt on screen means no press.
- The press cap is reached, then a BLOCKED notice names the pane.
- Wrapped or re-flowed text still matches.
- Tests use poll counts and fake clocks, no wall-clock budgets.
- `make validate` passes on the gated HEAD.

# Notes

Live deployment (services refresh/turnover) is the coordinator's job after
landing; the lane never restarts the watcher or bridges.

## Resolution

Implemented as designed: `rs/agentctl/src/paste_backstop.rs` holds the
normalisation, fuzzy match (whole text up to 80 normalised characters,
else first/last 40-character anchors), position and static-screen guards
and the delivered-prompt reader over the queue records;
`sweep_paste_backstop` in `rs/agentctl/src/agent_watch.rs` is the watcher
state machine (3 consecutive static polls spanning 45 s, at most 3 Enter
presses 20 s apart, every press noticed as `gen-backstop:`, then one
BLOCKED notice `gen-backstop-blocked:` naming the pane), wired into both
the full pass and the quick blocked check on the same screens as the
per-harness sweep, which runs first as a refinement and suppresses a
same-pass double press. Busy panes (a harness reader's running-turn
signal) and approval/permission/picker screens hold the backstop; foreign
text can never match because the match set is agentctl's own delivery
records. Production Enter goes through `ManagedAgents::press_enter`
(lifecycle lock, input-allowed and recipient checks, bare `Enter`, no
composer reading). Placeholder-only pastes (`[Pasted Content N chars]` /
`[Pasted text #K +M lines]`) have no screen text to fuzzy-match and stay
with the per-harness readers, as recorded in the #1561 matrix row values
sent to the matrix lane. Fake-pane tests cover Claude, Codex, Muse and an
unknown TUI shape (stuck, consumed, normal submit, foreign text, approval
hold, press cap then BLOCKED, echoed-and-working, echoed-and-finished,
wrapped/re-flowed text) with poll counts and fake times. Landed SHA and
gate evidence are reported to the coordinator on landing; live turnover
is the coordinator's job.
