---
title: 'documents-outline-text-list: Outline entries as an indented text list that stays open'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- documents
- ui
created_at: 2026-11-14T18:41:13.000000000+00:00
updated_at: 2026-11-14T18:41:13.000000000+00:00
---

# Description

[opus 5] Owner UI request 066 item 6. In the Documents reader, Outline
entries should be an indented text list with no button boxes: underline on
hover, a click jumps to that section, and the Outline pop-up stays open after
a click until the user clicks away outside it or presses Escape. Phone and
desktop. The outline tree model is #1541 documents nav work; this issue is
only the list rendering and the open/close behaviour.

# Design

- Rendering stays in `vibe-talk/web/src/view/documents/reader.ts`
  (`drawOutline`): entries remain real `<button type="button">` elements so
  keyboard (arrows/Home/End/Enter) and screen-reader behaviour (button role,
  `aria-current` for the current section) are unchanged; only their dress
  changes.
- Dress in `vibe-talk/web/styles/voice/21-documents.css` (`#doc-outline`):
  transparent background, no border or radius, indentation by heading level
  as before, underline on hover; the current section stays bold with an
  accent bar. The Outline button's title in `voice.html` says the list stays
  open.
- Open/close: an entry click only scrolls to its heading and leaves the
  pop-up open (focus stays on the entry); the existing outside-click handler
  and Escape handling (panel keys and the shell back level) are the only
  closers, beside the Outline toggle itself.

# Acceptance Criteria

- At phone (~390 px) and desktop widths the Outline pop-up shows an indented
  text list with no entry background or border; hovering an entry underlines
  it on desktop.
- Clicking/tapping an entry jumps to its heading, marks it current, and the
  pop-up stays open; a second pick jumps again; an outside click/tap or
  Escape closes it.
- Keyboard and screen-reader behaviour is kept: entries remain buttons with
  arrow/Home/End movement, Enter to jump, Escape to close, and
  `aria-current` on the current section.
- Unit tests cover the stay-open pick; the real-server browser test covers
  the text-list dress, hover underline, stay-open picks, outside close and
  Escape at phone and desktop widths.

# Notes

[opus 5] Sibling owner-UI lanes on the same page: #1540 footer links,
#1541 documents nav tree/scrubber (coordinated: that lane does not touch
`drawOutline` or the `#doc-outline` styles), #1543 reader click, #1544
desktop control bar, #1545 hamburger. Whoever lands second rebases and
re-gates. `vibe-talk/web/voice.js` and `web/src/app/main.js` are untouched.
Before/after screenshots were taken at 393 px and 1280 px against the real
server and inspected: before, entries are dark bordered boxes; after, an
indented text list that stays open over the jumped-to section.
