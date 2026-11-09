---
title: 'scrollback-typing-jump: the list jumps while reading scrollback and while typing'
status: open
priority: 0
issue_type: bug
assignee: mu-scroll
created_at: 2026-11-09T16:28:51.510479950+00:00
updated_at: 2026-11-09T20:27:02.193484259+00:00
---

# Description

Owner bug (reply 245, 2026-11-09 11:00 AM ET), on the installed vibe-talk PWA (probably Android Chrome), main channel view of a bridged chat instance:

"On this vibe-talk session right now I am getting some scroll back JUMPING while I am in the middle of typing. I need this root caused and fixed. WHENEVER I am back in the scrollback reading I NEVER want the screen to jump. The message I'm looking at should stay on the screen. And DEFINITELY when I'm typing I would expect nothing to jump away from my text box where I'm typing. Sometimes this particular effect happens where it scrolls back then jumps back to my typing (maybe when I keep typing?) but doesn't show the whole text entry window like it normally would."

Invariants to enforce and pin with tests:
1. While the reader is in the scrollback (not at the bottom), the screen never moves: the message being read stays at the same viewport position across new rows, re-renders, recounts, read-aloud updates, live refresh, and keyboard resizes.
2. While the reader types (composer focused), nothing scrolls the composer out of view.
3. Any snap back to the composer shows the WHOLE composer, never part of it.

Suspects at filing (to verify, not assumed): the on-screen-keyboard fit and its visualViewport re-triggers while typing; the at-the-foot test measured under keyboard geometry; anchor restore on new rows or re-render; composer growth compensation; the newest-count recount; drag-up refresh; read-aloud highlight scrolling; live-pill updates. Recent deploys touching layout, scroll, and the composer (the bottom-bar and chronological-documents changes, the side-cards/filter-kept-place moves, read-aloud) are in the bisect range.

# Design

Root-cause first, then the smallest fix at the writer that moves the list.

- Reproduce in a real browser: Chromium with mobile emulation at 360 px and 412 px with a simulated on-screen keyboard (visualViewport), plus desktop. Drive new-message arrival while typing and while reading scrollback. Instrument every write to the list's scroll position and every scroll-into-view call with a stack (a debug flag), find the writer, and bisect across the recent deploy range if needed. Name the exact commit and function.
- Fix so the three invariants hold by construction: no scroll writer may move a reader who is away from the foot; no writer may move the composer out of view while it has the focus; any deliberate snap to the composer shows it whole.
- Prefer the page modules over the legacy voice script. This fix owns the keyboard-fit and foot-room behaviour; the companion raw-message-page change to the same composer's scroll stop rebases on top of it (coordinate through the lead coordinator before editing shared composer code).
- Tests: a page test per invariant, plus a browser walk that drives new-row arrival, typing, and a keyboard resize, and asserts the reading anchor stays fixed and the composer stays fully visible.

# Acceptance Criteria

- Reading scrollback: with the list scrolled away from the foot, the anchored message's viewport position is unchanged (within a pixel) across a new row arriving, a re-render, a newest recount, a read-aloud update, a live refresh, and an on-screen-keyboard resize, at 360 px, 412 px, and desktop widths.
- Typing: with the composer focused and the keyboard open, typing (including suggestion-strip resizes) never scrolls the composer out of view and never moves a reader who has scrolled up to read.
- Whole composer: every deliberate snap to the composer leaves the whole composer (box and its action row as drawn) inside the visible viewport, never clipped by the keyboard.
- The root cause is named in Notes as commit plus function plus mechanism, with browser evidence.
- Page tests pin each invariant and a browser walk covers new-row arrival plus typing plus a keyboard resize; the full validation gate passes.

# Notes

Root cause (reproduced in Chromium at 412x915, instrumented; probe /tmp/mu-scroll-repro.py): LATENT, not a regression. The same failures reproduce identically on the pre-#1078 page (4564cc0f); no commit in the 2026-11-08/09 deploy range edits this scroll logic (moves verified word-for-word). Two mechanisms, one root - while a composer inside the list is focused, the browser, not the page, owned the list's scroll:

1. Typing at the bottom, the view jumps UP into older messages: a row at the foot laid out again taller after rendering (image/embed/streamed line settling), or the keyboard strip growing, increases the content (or shrinks the window) while the scroll offset stays put. The composer at the list's foot drops below the keyboard (measured: composer bottom 798 vs visible 579 after +180px growth; foot gap 8 -> 228, so the reader also stops counting as at the bottom), older messages fill the screen, and the next keystroke's browser caret-reveal brings back only the caret's line. Main had no content-growth foot keeper (the right column has one, #280; Main was left as #207 decided). No second mover was found: arrivals and re-renders held the anchor in the repro, read-aloud highlight makes no scroll call, and the live pill only relabels.

2. Typing from scrollback yanks to the composer: Chromium reveals the caret on text insertion (proven with zero page scroll writes, keyboard-free desktop geometry included). Per owner replies 266-268 that jump is acceptable with the composer focused, provided the whole composer shows; unfocused keys navigate and never type or yank.

Fix (first change): follow is one explicit state in Main's new foot keeper (listkeepers) - noted only from the reader's own scrolls and landings at the foot, never switched on by a caret reveal in the instant after an input; while a box in Main is being written in and follow is on, late growth is followed so the composer never sinks. After text goes in at the foot, the keyboard jump is made whole (keyboard.noteComposerInput), so any snap shows the whole composer on the keys. The foot every measure uses is the rows' foot: footGap/footTop leave out the keyboard's room under the fitted composer (footroom.keyboardRoom from keyboard.roomIn), so resting on the composer with the keyboard up counts as at the bottom and scrolled-up is measured without keyboard geometry. In scrollback (follow off) arrivals/re-renders keep the anchor as before (preservingScroll/settleAfterRead, verified in the repro focused and unfocused); only the Newest chip changes.

Tests in the first change: unit pins for the keeper's follow state (growth followed while writing, not while merely at the foot, off after the reader's own scroll up, never on from a reveal), for the whole-composer completion and roomIn, and for the rows'-foot measure. Existing page suites (scrolling, composer, keyboard) pass bar one pre-existing environment failure (process.threadCpuUsage missing in this shell's Node, #532 latency harness). Left for the second change: the maintained browser walk (phone + desktop widths, arrivals + typing + keyboard resize + late growth), desktop-width typing cases beyond the keyboard-free repro scenario, and the reply box. Slot scroll, branch mu-scroll/scroll-jump.
