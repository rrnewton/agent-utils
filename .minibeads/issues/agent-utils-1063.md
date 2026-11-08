---
title: 'collapsed-card-reaction-overlay: a collapsed card puts its reactions on the elision line, in its lower-right corner'
status: open
priority: 1
issue_type: feature
assignee: mu-reactcorner
created_at: 2026-11-08T13:31:00.000000000+00:00
updated_at: 2026-11-08T13:31:00.000000000+00:00
---

# Description

The owner (2026-11-08, on a phone in the All view): "These
collapse cards waste space on the emoji reactions. Here's my
proposal. For collapse cards render them as if they had no
reactions at all. Compute the layout in that way, then slap the
Emojis on the lower right corner on top of whatever else is
there. This usually should I imagine won't even collide the
text and we can experiment and see if it does. But text very
often wraps with a non-full line and we necessarily have at
least the vertical space from the line that already says XYZ
words elided."

Today a collapsed (folded) channel card draws its clamped
lines, then the elision note "(~N more words)" at the left,
then a separate row of reaction chips under it: one whole
extra line of height per reacted card, on the view the owner
skims.

# Design

- Collapsed cards only (`data-collapsed="true"`, with an
  elision line showing). The card is laid out as if it had no
  reactions; the existing strip (`.reactions`, same chips,
  same DOM order, same `role="group"` / `aria-label`, still
  labels a tap on which is a tap on the row) is then taken out
  of flow and drawn absolutely at the card's lower-right
  corner, on the elision-note line, whose right side the short
  note leaves free. Expanded and unfolded cards are unchanged:
  opening the fold restores today's separate row by the same
  `data-collapsed` switch that shows and hides the note.
- Never cover text. A strip is an overlay candidate at draw
  time only when it is small enough to plausibly share the
  note's line (few chips, no custom-emoji names). After the
  row is on the page the candidate is measured for real: the
  note's drawn width, a clear gap, and the strip's drawn width
  against the card's inner width, and the strip against every
  other drawn block in the row. Any overlap — a long note,
  many reactions, a narrow phone, a large text size, a summary
  or reply line under the text — demotes the strip to today's
  separate row. Demotion only: a measured pass never promotes,
  so the check cannot oscillate; a redraw re-decides.
- Code lives where the strip is drawn
  (`web/src/view/channel/channelrows.ts`) and in the
  channel-rows stylesheet (`web/styles/voice/12-channel-rows.css`).
  No `voice.js` change, and no change to the fold owner
  (`view/channel/folding.ts`): the overlay keys off the
  `data-collapsed` / `data-summarised` attributes it already
  maintains, and off the `:root[data-collapsed-lines="0"]`
  state, where there is no elision line to share.

# Acceptance Criteria

- A collapsed card with two ordinary reactions is exactly as
  tall as the same card with none, its chips at the lower
  right on the elision line, clear of the note and of every
  control, each chip's centre hit-testable.
- A collapsed card whose reactions would reach the note (many
  chips, a custom-emoji name, a narrow width) keeps today's
  separate reactions row; no chip ever covers text.
- Expanded and unfolded cards draw their reactions exactly as
  today; reading-aloud (DOM) order and the strip's group
  label are unchanged everywhere.
- A page test pins the overlay marking and the fallback
  decision; the existing real-browser reactions check is
  extended to folded cards with zero, two and many reactions
  at phone (412px) and desktop widths.

# Notes

Owner request of 2026-11-08 for the next vibe-talk deploy.
The elision note is "(~N more words)" from `#244
elision-word-count`; the strip is `#219 emoji-reactions` /
`#312 slack-reactions`. Stays clear of `#1052
post-to-main`'s reserved reply ranges and of `#1057
newest-count-chip`'s `folding.ts` / `scrollkeep.ts`.
