---
title: 'short-ref-emoji-render: :thread:NNNN and :email:NNNN draw as emoji and number'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-15T17:22:00.000000000+00:00
updated_at: 2026-11-15T17:22:00.000000000+00:00
---

# Description

Owner UI request 100 (fast-tracked): in vibe-talk a short reference from
`#1565 vibe-talk-short-ref-links` reads as `:thread:060` with its colons,
where Slack draws 🧵060. The general emoji rule (`slack/emoji.rs`) leaves a
shortcode that touches a letter or digit as written, which is right for
`10:100:20` and paths, so `:thread:1234` never became its emoji on any path,
resolved or not.

- A resolved `:thread:NNNN` or `:email:NNNN` keeps its link (`data-msg`,
  `data-shortref`, same address and tap behaviour) and is drawn as its emoji
  and number: 🧵 and ✉️, the standard emoji `thread` and `email` name.
- An unresolvable one is drawn the same way, unlinked, under the existing
  `shortref-unknown` dotted underline.
- `:issue:NNNN` has no standard emoji, so it keeps its written form, linked
  or dotted as before (coordinator's decision on this lane).
- Any other shortcode touching a digit stays literal (`:crab:123`), and a
  reference in code stays exactly as written.
- Only what is drawn changes: Copy text still copies what was written.

# Design

One display point: `render::link_short_refs` already makes every short
reference a link or a marked span for live, cached and refreshed bodies
alike, so its label (`render::short_ref_label`) is the only change: thread
and email labels become emoji plus the digits as written, issue labels are
unchanged. Addresses, marking and page navigation are untouched, as are
`web/voice.js` and `web/src/app/main.js`. `SearchText::VERSION` bumps to
`render-7` with its own pinned corpus digest, so indexed drawn text is
rebuilt. Sibling lane #1606 also edits `render.rs` and also takes
`render-7`; the lane that lands second rebases, takes `render-8` with its
own pinned entry, and re-validates (coordinator's rule).

# Acceptance Criteria

- Unit tests: resolved thread/email/issue draw as marked links with emoji
  labels (issue unchanged); unknown ones draw as emoji under the dotted
  marker with no link; references in code stay text; other shortcodes
  touching digits stay as written; the emoji reader's own rule still leaves
  `:thread:1234` and `:crab:123` literal.
- Browser test at 390 px and desktop (`tests/short_refs_browser.py`, real
  Chromium): the emoji-labelled links still highlight and drill in, the
  unknown reference is no link; screenshots at both sizes, looked at.
- No touch to `vibe-talk/web/voice.js` or `web/src/app/main.js`.

# Notes

[opus 5] mu-refemoji. The headless test host has no emoji font, so the
screenshots show placeholder boxes where the emoji go; the DOM text the
walk asserts is the emoji itself, and real devices draw it. Speech still
says a reference from the written text, as before; only the drawn body
changed.
