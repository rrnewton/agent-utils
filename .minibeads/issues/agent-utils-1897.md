---
title: 'agentctl-claude-narrow-pane-typed-not-submitted: a typed prompt in a narrow pane is never submitted'
status: closed
priority: 0
issue_type: bug
labels:
- agentctl
created_at: 2026-11-19T00:00:00.000000000+00:00
updated_at: 2026-11-19T00:00:00.000000000+00:00
---

# Description

Five Claude Code lanes, each in a narrow Herdr pane (about 42 columns),
had an agentctl message typed into the composer but never submitted
(2026-11-19, about 2:14–2:49 AM ET). The sender build already contained
#1683 pane-lock-held-by-stuck-sender (bounded pane lock and lower-region
recovery) and its delayed re-check. Every stuck record has the same
shape: queued, then stranded 12 s later (the paste was not observed
inside the in-lock stage window), then every later delivery to that pane
failed for minutes with "does not show a recognisable claude composer
after 15s; nothing was typed", until a person pressed one Enter per
pane, which cleared the composer and confirmed the whole backlog.

The prompts begin with a long sender-header line that wraps at narrow
width, and the composer showed the message tail above its separator
line. Neither #1683's lower-region recovery (`recover_typed_draft`) nor
#1560's watcher stuck-paste backstop (Enter after 3 polls over 45 s)
fired. Record fields on the stuck deliveries: `possibly_submitted`
true, `stranded_passes` 1, `delivery_attempts` 0.

# Design

- Reproduce at pane widths around 30, 42 and 80 columns: a tall prompt
  drawn in a narrow composer scrolls its first row (and its header)
  out of the visible frame, leaving only the prompt tail above the
  composer's bottom rule.
- The lower-region match must recognise our own typed prompt from the
  visible tail alone: a long prompt whose head anchor has scrolled off
  still matches when a long, exact tail run of the prompt is visible
  and ends near the bottom of the region. Foreign text, which cannot
  contain that exact run of a delivered prompt, is never submitted.
- The in-lock delivery stage must act on that same recognition when no
  composer reader parses the screen: our just-typed paste, seen static
  in the lower region at the prompt's tail, gets one verified Enter
  instead of being left stranded after the stage window.
- Both editions keep the same rule (Rust `paste_backstop` /
  `submission`, Python `submission` region match), and the harness
  support matrix records the behaviour.

# Acceptance Criteria

- At widths 30, 42 and 80, a typed-but-unsubmitted prompt of our own
  queue is recognised and submitted with one verified Enter; it is
  never typed twice.
- A foreign draft at the same widths gets no Enter.
- The watcher backstop and the delivery-path recovery both recognise
  the tail-visible shape, in the Rust edition; the Python edition's
  same-prompt region flush matches it too.
- Tests use condition-based waits (poll counts / fake clocks), never
  wall-clock budgets.

# Notes

Related: #1683 pane-lock-held-by-stuck-sender, #1560
harness-agnostic-stuck-paste-backstop, #1546, #884, #720, #1691
(second Enter, open), #1732, #1773, #1882 (false-failed deliveries, a
neighbouring lane in the same file area). No lane, host or person is
named here.

## Resolution

[watermelon-full-preview] Landed b9f5dc99 on github/main (owner-priority
gate: 62 steps passed on the pre-replay commit, ledger head_sha matched;
the support matrix had also moved on main, so the replayed combined
tree was spot-checked before pushing). Root cause: a tall prompt in a
narrow pane scrolls its head and composer row out of the visible frame,
leaving only its tail above the separator — no composer reader parses
that screen, and the lower-region match required the head anchor, so
the stage wait stranded the paste, later deliveries refused the pane,
and neither recovery nor the watcher backstop recognised it. Reproduced
live on a real Claude Code in a 42x12 pane in a Herdr pane created for
the purpose: tail-only screen, composer unreadable, head anchor absent,
and one Enter submitted the prompt. The region match now accepts a
long exact tail run (at least 48 normalised characters, longest first)
when the head has scrolled off, in the Rust backstop, the delivery-path
recovery, the in-lock stage wait, and the Python region flush; a fresh
tail-only paste is submitted in its own delivery with one verified
Enter, never retyped, and a foreign draft at 30, 42 and 80 columns gets
no key. Tests at 30/42/80 in both editions plus a watcher case
(condition-based waits throughout). No persisted queue or record field
changed. Live deployment is the coordinator's batch.
