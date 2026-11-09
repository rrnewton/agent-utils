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

## Batch 2, 2026-11-09 (mu-settle)

[muse watermelon] `keyboard_browser.py`, `composer_key_browser.py`,
`search_chord_browser.py`, `keyboard_hides_bar_browser.py` and
`fold_latency_browser.py` (75 `settled()` sites reviewed). These files have two
different helpers. `Walk.settled` (keyboard, shared with composer-key and
search-chord through `Walk`) is already a condition wait — the scroller's
position unchanged over four samples — and stays for scroll positions and
geometry a scroll settles into; its docstring now says what it cannot see (a
key's focus, screen or scroll-start effect beginning after its quiet window),
and `Walk` gained `wait_for(condition, why)` and `key_until(chord, condition,
why)` in `Desk.wait_for`'s shape for those effects. `Desk.settled` here is
batch 1's two-frame helper (hides-bar, fold-latency).

Converted (each waits on the expression the following check reads, today's
checks and messages kept after):

- keyboard: Home/End and Ctrl+Home/End wait on the scroller reaching its end;
  Settings' PgDn and PgUp in the message box wait on the scroll crossing half a
  screen (the delayed-start flaw `moved()` already guarded against for paging);
  `/`, Escape, the Ctrl+S chords (`to_box`, from the search field, on the call's
  view), Settings open/leave, Ctrl+K and `m` wait on the focus or screen they
  then check; the Settings Tab walk waits until the reached stop is brought
  onto the screen; `e` and Ctrl/Cmd+E wait until the marked row shows Done;
  `one_lit` with an expected row first waits until that row is the lit, focused
  one (the previously lit row is also exactly one lit row); the four Enter
  open/close checks in single-selection wait on the row's `data-collapsed`.
- composer-key: the central `chord()` waits for the named box to hold the focus
  and for this chord to have been heard (non-vacuous when the box already had
  the focus, the draft case); `select_row` waits for a row to hold the focus.
- search-chord: the central `searched()` waits for the search drawn, its field
  focused, on the main screen; `close_search` waits for it hidden; the ⋯ menu's
  focus, End's selected row, the typed slash in the box, and Help on a Mac wait
  on the expressions read next.
- keyboard-hides-bar (9 of 27): blur waits for the keyboard said closed and both
  bars drawn; the new-line taps wait on the box's resulting value, focus and
  keyboard state; the iOS steps wait through the new `wait_ios_flush` until the
  box stands where `ios_flush` holds it (keyboard up, panned, sent to the foot,
  after a send — the fixed 500 ms there is gone — and short-innerHeight), the
  control bar gone, and the keyboard closed.
- fold-latency: `select()` waits until the row is the selected one
  (`data-selected`), the settle after it kept to flush the scroll's paint
  outside the measured presses.

Kept, with the reason in each case:

- `Walk.settled` before scroll-position, stillness and moved-by-a-pixel checks,
  scroll baselines (`spot`, the ring's list-follows reads), and after exact
  `wait_for_function`/`wait_for_selector` pins (view keys, thread open/close,
  two-columns): stillness is the condition those checks read.
- The self-correcting reach-a-row loops (press until `ring()` names the row)
  and Tab/Shift+Tab focus stops: focus moves inside the dispatch the CDP press
  waits for, and each loop re-reads until it is true.
- Negative windows: a bare `/`, a bare digit and AltGr changing nothing (a
  condition wait would pass before the key travels); focused with no keyboard
  yet in hides-bar; no-movement reads after viewport changes and pans.
- Timed durations that are the subject: fold-latency's press pacing (four
  presses 60 ms apart), its trace/keys deadline polls, the 1000 ms unpack soak
  and the warm-up key's settle (all outside the measured interval, so the
  presses are measured from a steady page); the held-Ctrl+Enter 400 ms and
  Cmd+E out-of-sight 200 ms no-send windows; hides-bar's `SETTLE_WAIT_MS`
  no-change windows and the wheel's 300 ms landing time.
- Mid-cycle and between-action settles (fold-latency's unpack clicks, the
  Show-read mode cycle), and load baselines after `wait_for_selector` where
  only a screenshot follows.

Solo on 2026-11-09 (Chromium 149.0.7827.55): all five files pass whole.

## Batch 3, 2026-11-09 (mu-settle)

[muse watermelon] `search_dates_browser.py`, `search_unmatched_browser.py`,
`view_switch_browser.py`, `filter_place_browser.py` and
`search_whose_browser.py` (54 `settled()` sites reviewed). These files were
already condition-first — each carries its own local `until()` /
`wait_rows()` bounded poll, and most settles follow one — so this batch mostly
records that, documents each local two-frame `settled` helper's remaining use
(a paint frame after its `until`, before geometry, a parked-scroll baseline,
or the end of a no-move window), and converts the sites where the bounded wait
pinned less than the check then read:

- search-dates: Mine and back-to-everyone now also wait until the showing rows
  and the count say the filter has been applied (the button's `data-author`
  flip alone did not); four settles that gated nothing are gone (one after a
  synchronous scrolled measurement already read, one superseded by the next
  park's settle, and two duplicates of the scroll-reset settle that follows).
- search-unmatched: a new `until_shown` waits for exactly the rows `shown()`
  will then be held to — the opened run above 209, harbor's card alone, the
  opened card run, and Main under "bright" (replacing an any-non-empty count
  wait); the thread list waits until its one shown card is harbor's; the
  unpacked lantern thread waits for its hit and its seam line, and the opened
  run for its messages; the server-search scope line is read through
  `server_settled()` instead of a bare settle.
- view-switch: one genuine gap — behind its held rows, All's read having been
  sent did not say its freshness pill had rendered; it is now polled for
  before the kept-place geometry.
- search-whose and filter-place: no call-site conversion. In whose, every
  press is already followed by its exact `until` (chip words, tooltip, fade),
  and the press helper's settle must stay short: the chip it summons is up for
  about a second and a half. In filter-place every settle follows an exact
  `until` (paint before a moved-by-a-pixel read), lands a parked scroll before
  its baseline, or ends the deliberate 500 ms nothing-moved windows, which are
  the check and stay.

Kept elsewhere, as in batches 1–2: paint-after-exact-wait before rows, seams
and bar geometry (waiting on the measured rect would wait for the assertion);
parked-scroll baselines; mid-cycle press settles whose final state the
following `until` pins; and pre-press settles where nothing is asserted in
between.

Solo on 2026-11-09 (Chromium 149.0.7827.55): all five files pass whole.

## Batch 5, 2026-11-09 (mu-settle)

[muse watermelon] `thread_card_focus_browser.py`,
`thread_card_header_browser.py`, `thread_card_fit_browser.py`,
`thread_card_max_lines_browser.py` and `thread_card_markdown_browser.py`
(70 `settled()` sites reviewed). Four use batch 1's `Desk`; fit has its own
`Walk.settled` (two frames and 150 ms), now documented as a
paint-and-restyle window to use only after a `wait_for`. Like batch 3, most
sites already follow an exact wait (a card state, a highlighted attribute,
row counts) and the settle paints before card geometry, computed colours and
screenshot pixels; those stay, including the double settles before pixel
sampling and `whole_links`' scroll scan (the links redraw in the page's own
frame, and the scan's wanted count varies by caller).

Converted, today's checks kept after each:

- focus: a highlighted or tapped-whole card now also waits until its text
  measures unclamped past three lines (the `STATE_JS` lines expression), and
  a tapped card on the phone until it says what its thread is about — an
  attribute flip alone did not say the whole text had been drawn.
- header: Main at its foot waits for the line clipped `up` and selected;
  the focus-lost redraw case waits for the focus to be the body's and for the
  selected row to have actually been redrawn; three settles that gated
  nothing (before `watch`/`open_card`, which carry their own waits, and a
  duplicate in the thread list) are gone.
- fit: a focused read card waits for its computed opacity and filter to come
  back; Unpack all waits for the read thread's rows; Fold read waits for its
  run line, Hide and Show read for the read cards to go and come back.
- max-lines: the packed premise waits for the card collapsed and split
  (`sign_in` and `to_threads` pinned neither); at K = 2 it waits for the
  continuation the next check reads, and a link landing waits for the
  continuation under the landed card.
- markdown: no call-site conversion — its settles follow exact waits (the
  summary's arrival, the build chip) or land a scroll before a moved-by
  baseline; its frame-count and load-timing windows are the measurements.

Kept: the flight and glide timings in header (the 1.5 s hover glide, the
2.5 s mouse-crossing stillness samples, the per-frame flight recorder) and
markdown's frame/load windows — durations that are the subject; highlight
absence and other no-change checks; mid-cycle Unpack and read-mode clicks;
scroll baselines before moved-by-a-pixel reads.

Solo on 2026-11-09 (Chromium 149.0.7827.55): all five files pass whole.
