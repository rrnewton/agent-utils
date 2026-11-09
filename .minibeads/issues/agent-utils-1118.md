---
title: 'browser-fixed-frame-settle: browser checks assert after a fixed two-frame wait that loses to paint timing under load'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T09:34:09.000000000+00:00
updated_at: 2026-11-09T09:34:09.000000000+00:00
---

# Description

On 2026-11-09 the full gate on the #624 voice-page-modularity entering-a-thread
commit (55fa8bc4) failed vibe-talk.check.threads-polish-browser once:
"Fold read by h drew no run line in All: []" (read_mode_walk,
vibe-talk/tests/threads_polish_browser.py). The step had never failed in the gate
ledger before. It passed 5 of 5 solo reruns on the same commit, and the moved code
is not on that path.

The cause is the wait, not the page. The walk presses h, calls desk.settled(), and
then reads the drawn run lines. settled() (Desk in desktop_layout_browser.py, and
local copies in many other browser tests) is exactly two requestAnimationFrame
ticks with no condition. The run line is drawn by a render pass after the mode
change. Under full-gate load (about 150 steps with browsers in parallel), two frames
can pass before that pass paints, so the check reads an empty list while the mode,
label and status already say Fold read.

desk.settled() appears about 350 times across about 30 browser test files, and
several files define their own settled(). Other load-sensitive browser steps seen
in gates may share this pattern.

# Design

- Replace fixed-frame settles that precede a check with a bounded wait on the
  condition the check reads, using the existing Desk.wait_for(condition, why,
  seconds) or an equivalent page-side poll. For the failing walk: wait until the
  run line exists (bounded), then assert.
- Keep settled() only where nothing is asserted on the next render, or where a
  frame boundary is itself the point. Document that use at the helper.
- Sweep the browser harness for other fixed-frame or fixed-timeout waits that
  come before an assertion, and convert them the same way. List the files touched
  and any waits deliberately left alone, with the reason.
- Keep the bounds generous enough for a loaded gate host, and fail with the same
  messages as today.

# Acceptance Criteria

- read_mode_walk waits on the run line, not on two frames.
- The sweep's list of converted waits and deliberately kept settles is in Notes.
- The touched browser steps pass in a full gate and in solo reruns.

# Notes

Filed by the devbig030 coordinator at the lead's request, after the entering-a-thread
landing (0c621bd8). Gate evidence: the ledger entries for 55fa8bc4 (one gate fail,
five exclusive-rerun passes).

## Batch 1, 2026-11-09 (mu-settle)

[muse watermelon] The whole of `threads_polish_browser.py`, `desktop_layout_browser.py`
and `two_column_browser.py` (61 `settled()` sites reviewed). Both `Desk.settled()`
helpers now say their legitimate use at the helper: a paint frame after a `wait_for`
has pinned the state, before a geometry or screenshot read, where nothing is asserted
on the next render, or where frames passing is itself the point; never the only wait
before reading what a render draws. Neither helper's behaviour changed (the
two-column `Desk` is imported by some 30 other suites).

Converted (each waits on the expression the following check reads, and the check,
with today's message, still follows):

- threads-polish `read_mode_walk` (the failing walk): `h` to Fold read waits, 30 s
  bound, on mode, label, status and the run lines themselves (`READ_LINES_JS`, the
  expression `READ_MODE_JS` reads); a tap on the fold line waits on lines empty and
  all three cards; `h` to Hide read waits on mode and the runner's card alone; `h`
  to Show read waits on mode and all three cards (`READ_CARDS_JOIN_JS`). Each is a
  real transition: the pre-state is checked just above (for example the fold line
  reads "… 2 read threads" before the tap waits for no lines).
- threads-polish `center_walk`: All drawing rows 220 and 203 replaces the settle
  after the picker wait. `compose_walk`: the three Cmd+S chords wait at their call
  sites on the focus and view they then check (the `chord` helper keeps its settle;
  it serves six unlike sites). `back_walk`: the Cmd+[ and Cmd+] loops wait via
  `wait_where`, the same fields `where` reads, and Forward past the end waits for
  the status sentence it then checks. `order_walk`: `d` waits on the card order and
  the chip's label, a reload waits on the remembered order (replacing its settle),
  and the press back waits on the latest-update order. `pack_walk`: Unpack unread
  waits on `data-unpack` and the chip's words; the triangle's hover waits on its
  wash's computed background and its focus on the ring's computed outline.
- two-column `columns_walk`: a highlighted card waits until its text measures five
  lines (the `CARDS_JS` expression), and scrolling both columns to their feet waits
  on both scrollers arriving, replacing the settle. `older_walk`: the walk's end
  waits on the three found matches and the note's opening words (the fixed 200 ms
  is gone). `replies_walk`: typing the search waits on the no-threads sentence and
  the replies control being offered, and the walk's end waits on three shown cards
  and the note (200 ms gone). `replies_place`: the search waits on the ten root
  cards shown (400 ms gone), and the reader's scroll into the cards waits until the
  anchor card stands 200 px under the column head (200 ms gone). `advance_walk`:
  the first sentence replaces a settle plus 300 ms, and each press waits on its own
  searched-count sentence and note (200 ms gone).
- desktop-layout: the fixed 200 ms after scrolling Main to its foot is now a wait
  for Main's rows drawn, the agent's and the owner's among them; the Settings head
  waits until its host line and buttons have laid out, returning to one column
  waits for the reading-width cap to be back, and a thread on the right waits
  until its rows are drawn in a column of some width. The settles stay after
  these as the paint frame before the `HEAD_JS` / `COLUMNS_JS` geometry.

Kept, with the reason in each case:

- Settled after an exact `wait_for`, before `getBoundingClientRect` /
  `getComputedStyle` / selection measurements or a screenshot (most of
  desktop-layout, the two-column layout and cards reads, the threads-polish chip
  looks): the state is already bounded, the frame is for paint, and waiting on the
  measured value itself would destroy the measured-value diagnostics.
- Click-cycle mid-steps (`set_chip`, `unpack_to`, the read-mode button cycles):
  a control passes through intermediate states, so its final state cannot be
  waited on mid-cycle; the final state is pinned by a `wait_for` or check after
  the loop.
- No-change checks (the bare `]` in `back_walk`, no highlighted card after card
  clicks in `pack_walk`): the state is already true before the key travels, so a
  condition wait would pass vacuously; frames passing is the test.
- Scroll baselines (a row's centre after `scrollIntoView`, the replica card's top
  before packing, the place-preservation window): no DOM value changes, and the
  settled layout is the baseline the tolerance check compares against.
- Between-action settles where the next action carries its own bounded wait, and
  cleanup settles nothing reads (blur and mouse-away in `pack_walk`).
- Fixed durations that are the subject, not a render wait: the column links'
  1.5 s glide and its reduced-motion step, the no-read-by-itself windows
  (300 ms in `older_walk`, 400 ms kept in `replies_walk` after its new condition
  wait), the place-preservation 300 ms, and screenshot-only fades
  (`replies_walk` 500 ms, desktop-layout thread walk 300 ms).
- `frame()` and `settled_links` in two-column: the page redraws the links in its
  own frame on scroll, and `settled_links` already polls a real condition (the
  drawn links equal the pairs on screen).

Solo on 2026-11-09 (Chromium 149.0.7827.55): all three files pass whole —
threads-polish at both sizes including `read_mode_walk`, two-column including the
older, replies, place and advance walks, desktop-layout at all its sizes.
