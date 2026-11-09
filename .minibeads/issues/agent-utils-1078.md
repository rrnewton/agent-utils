---
title: 'bottom-bar-switcher: outline in the bottom bar, dock rows flipped, tap-through tabs at the lower right'
status: closed
priority: 1
issue_type: task
created_at: 2026-11-08T17:56:40.954836998+00:00
updated_at: 2026-11-09T00:38:57.266806077+00:00
closed_at: 2026-11-09T00:38:57.266805977+00:00
---

# Description

Owner reply 231 (2026-11-08), from the Documents reader on a phone with
the document outline pop-up open. Today the reader top bar is Back,
title, Outline and Read, the outline rows are filled light-blue
buttons, and the Documents bottom bar ends with the view switcher
followed by Fetch at the lower right.

The owner asks for six linked changes. The outline pop-up should have
a border with light text on a dark background, in a slightly different
style from the main window so it reads as an overlay. The Outline
button should sit on the bottom bar so it stays reachable while
scrolled. Fetch and the view switcher should swap so the switcher is
the lower-right button, and the switcher must stay in the same
physical place while tapping through Voice, Channel and Documents.
The two control rows on the voice and channel views should flip, so
the reader controls sit above and the bottom bar is always at the very
bottom; hiding the reader controls in the channel view must not move
the bottom bar. While tapping through views, the vertical chip stack
above the switcher should become a horizontal row of three cells,
Voice, Channel and Documents, with the highlight moving left to right
like browser tabs, disappearing shortly after tapping stops.

# Design

Outline styling is stylesheet only, in the Documents stylesheet: dark
panel and rows, light text, a border on each row and no filled accent
background, visually distinct from the main window in both colour
schemes. The current-section row keeps a clearly distinct mark in
addition to its existing programmatic current marker, and keyboard
focus stays visible. Reading order, heading levels, arrow-key
movement, jump on activation and Escape handling inside the pop-up do
not change.

The Outline control keeps its identity and moves in the shell markup
from the reader toolbar into the control bar beside Fetch. It is shown
only while the Documents reader is up with headings to list, gated by
stylesheet visibility on the visible pane plus the reader's existing
headings fact, and it toggles the same pop-up. Focus return after a
jump or Escape goes to the moved control without scrolling the
document. Fetch and the view switcher swap places in the same markup
so the switcher is last in the bar.

The dock rows flip in markup order so the control pane (the reader
controls among the large controls) comes first and the control bar
(gear, pickers, actions, Fetch, Outline and the switcher) is last, at
the viewport bottom in every view. Stylesheets that assumed the old
adjacency, the keyboard-open rules, the reader-hidden rules, the
desktop regime and the read-strip anchoring are updated to the new
order. Tab order follows the new visual order. With the bar placed at
the top by setting, its drop-ups open downward as the existing trays
already do. Hit sizes, accessible names, safe-area padding and
keyboard shortcuts do not change.

The view selector keeps its rotation and its press handling. Its
pop-up becomes one horizontal row of three equal cells in rotation
order, Voice, Channel, Documents, anchored above the switcher (below
it when the bar is at the top). The current view's cell is highlighted
and marked current; each cell still chooses its view directly by tap
or keyboard activation. The row appears on a switcher press and hides
600 milliseconds after the last press, the same delay value as today,
now measured from the last press so rapid tap-through keeps it up; a
key press or a tap elsewhere still dismisses it at once.

Planned files: the shell markup, the dock, control-bar, control-pane,
desktop and Documents stylesheets, the Documents selector and reader
modules, the Documents view composition for button visibility, and the
page and module tests that price or drive the bar. No change to the
reply composer, to the shell navigation orchestration, or to the
global keyboard maps is planned; the pending outline keyboard work
owned elsewhere keeps its files.

# Acceptance Criteria

- The outline pop-up shows bordered rows with light text on a dark
  background in dark and light schemes, with no filled light-blue
  rows; the current section is unmistakable at a glance and to a
  screen reader, and arrow keys, Home, End, activation and Escape
  behave as before.
- With a document open and scrolled, Outline is present in the
  Documents bottom bar and opens and closes the pop-up; it is absent
  from the reader top bar, from the Documents list and from the other
  views, and absent for a document without headings.
- In the Documents view the bar ends with Fetch then the view
  switcher at the lower right.
- At 360, 412 and 1280 widths in dark and light, the bottom bar and
  the switcher measure at the same position in Voice, Channel and
  Documents, including with the reader controls hidden in the channel
  view and with the control pane hidden in Documents; tapping the
  switcher cycles Voice to Channel to Documents to Voice without the
  thumb moving.
- While tapping, a horizontal Voice, Channel, Documents row shows
  above the switcher with the highlight stepping left to right and
  wrapping, then disappears shortly after tapping stops, leaving the
  chosen view up; direct choice from a cell still works.
- Focused page and module tests cover each item, and browser checks
  with screenshots at the three widths in both schemes are attached
  to this issue.

# Notes

Owner reply 231, 2026-11-08 12:27 PM ET. Sequenced to avoid the
in-flight voice-page modularity work and the reply-composer change:
the plan needs no edit to the legacy page script; if read-strip
anchoring proves otherwise during implementation, the exact function
and line range will be cleared with the coordinator before any edit.
The outline keyboard remainder (a toggle chord and Escape ordering in
shell navigation) stays with its current owner; this change preserves
the reader's own Escape and focus behaviour so that work can layer on
top. Screenshots from the browser checks will be added here.
