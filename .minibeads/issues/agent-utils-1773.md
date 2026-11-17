---
title: 'agentctl-muse-placeholder-hint-as-draft: Muse placeholder hint read as a foreign draft holds every queued message'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-17T08:45:08-05:00
updated_at: 2026-11-17T14:14:41-05:00
closed_at: 2026-11-17T14:14:41-05:00
---

# Description

[watermelon-full-preview] Owner rule: every stuck composer is an
agent-utils bug. An idle or mid-run Muse composer shows a greyed
placeholder suggestion such as "Ask to monitor a running build or log
and get pinged on changes" (63 chars) or "Tab queues or steers while
a run is active". agentctl's composer reader takes it as a foreign
draft: "the Muse editor holds a draft that agentctl did not type
(63 characters)". Every queued message is then held (10+ minutes,
sometimes hours) until someone types into the pane directly. Seen
today on at least five lanes on two teams.

Related: #720 agentctl-muse-stale-draft, #1085
muse-send-staged-unsubmitted, #1732
agentctl-muse-midturn-typed-not-submitted (Muse draft handling),
#1500 codex-idle-animation-reads-as-draft (the Codex idle animation
was read as a draft: same class of bug, so reuse its approach),
#1683 pane-lock-held-by-stuck-sender, #1602
codex-capacity-live-screen (Codex composer placeholder).

# Design

Fix, in this order of preference:

1. Use styling. If herdr can read the pane WITH SGR attributes
   (`herdr pane read --format ansi`; `read_screen_with_cancellation`
   already uses it), treat composer text rendered dim/faint/grey
   (Muse's placeholder style) as empty. Typed text is
   normal-intensity.
2. If no styled read exists, recognise Muse's placeholder hints
   structurally: the composer line is exactly one hint, with the
   cursor at column 0 before it, and it matches the hint set taken
   from the installed Muse. Version the set, keep it in one table,
   and treat an unknown long text as a draft, as today (fail closed:
   never type over a real human draft).

Either way, a real draft (anything typed, including agentctl's own
staged paste) must still be detected. The agent watcher's "held"
notice uses the same classifier.

# Acceptance Criteria

- Deterministic tests from captured screens: both hints, idle and
  mid-run (a "Backgrounded task running" footer), a real human
  draft, and an agentctl staged paste.
- A Muse editor holding only a placeholder hint reads empty:
  idle when no turn runs, working (not staged) when one does; no
  "did not type" refusal, no held notice.
- A real draft and a staged paste still read as drafts and are
  never typed over.
- `make validate` passes on the gated HEAD.

# Notes

Live styling (coordb evidence, stuck seat): the hint is truecolor
grey `38;2;103;108;116` (the status line's grey) after a blue glyph
`38;2;90;160;255`, not SGR faint. The ANSI proof therefore matches
that grey (or the screen's own status-line foreground); faint still
counts. The hint table holds every Muse 1.4.23 `prompt_hint.rs`
ghost, including "Type @ to search and insert workspace file paths".

Second symptom on the same seat, split to a follow-up (number from
coordb): later sends report "cancelled while waiting for a lock of
queue ...; nothing was typed: cannot invoke Herdr: control command
was cancelled" under `timeout 40`, with no lock holder in `fuser` or
`/proc/locks` and the item still in `inbox/` while status said
"staged, nothing pending". The drain maps target-lock/readiness
Herdr errors through its queue-lock label, so a composer wait can
be blamed on a lock wait; the follow-up should name the real wait
("composer holds a draft agentctl did not type: <first 40 chars>")
and count `inbox/` items as pending in status. The hint fix removes
the staged wait itself; the labelling fix is separate.

## Resolution

[watermelon-full-preview] Landed e977291a on github/main (gated
bd87040b, owner-priority gate, validate OK). The Muse editor reader
treats an exact, wrapped or edge-truncated Muse 1.4.23 hint (one
versioned table, Rust and Python pinned identical), or grey/faint
ANSI text, as an empty composer; the watcher sweep shares the
reader. Tests cover both reported hints idle and mid-run, narrow
40/60-column wrap/cut, the live truecolor sequence, a real draft
and a staged paste. The lock-labelling symptom is #1775
agentctl-drain-composer-wait-labelled-lock-wait.
