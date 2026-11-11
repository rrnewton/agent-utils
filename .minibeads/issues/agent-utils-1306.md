---
title: 'doc-star-gold: a starred document shows a filled gold star, not a blank button'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
created_at: 2026-11-11T15:28:12+00:00
updated_at: 2026-11-11T15:28:12+00:00
---

# Description

[watermelon-full-preview] Owner reply 363 (2026-11-11 10:21 AM ET): "when I click the Star icon I expected it to fill in the outline with a gold star. Instead the button went blank."

Verified on github/main: the card star (`vibe-talk/web/src/view/documents/list.ts`, `.doc-star`, glyph `★` when starred else `☆`) and the reader header star (`#doc-star` in `reader.ts`) are plain `<button>` elements. The generic button rule in `vibe-talk/web/style.css` gives every button `background: var(--accent)` with white ink, while `vibe-talk/web/styles/voice/21-documents.css` sets `.doc-star[aria-pressed="true"] { color: var(--accent, Highlight) }` (and the same for `#doc-star`). A starred star is therefore accent-blue ink on an accent-blue ground: invisible, so the button looks blank.

# Design

- In `21-documents.css`, give `.doc-star` and `#doc-star` their own ground and ink instead of inheriting the generic accent button ground: an outline star in ordinary ink when off, and a filled star in gold when on. Gold is the page's existing pin gold (`--pin-ink` / `--pin-bg` in `00-frame.css`, defined for both colour schemes), with a literal fallback.
- Keep the glyph contract (`★` starred, `☆` unstarred) and `aria-pressed`; no behaviour change to starring, filtering, or persistence (`#1265 documents-star-hide`).
- Add a browser test that the starred state is visibly different (glyph and computed colour, and colour different from the button's own background) at phone (393x852) and desktop (1280x800) sizes, waiting on conditions not wall-clock budgets (owner rule 346).

# Acceptance Criteria

- Starring a document card shows a filled `★` in gold; unstarring shows the outline `☆`; the reader header star matches.
- The new browser check passes at 393x852 (touch) and 1280x800 and would fail on the old blue-on-blue CSS.
- Existing documents browser and view tests stay green; the fix is gated at owner tier and landed on its own.

# Notes

Owner reply 363, fast-track. Filed by mu-docfetch before implementation; the fix lands separately on mu-docfetch/doc-star-gold, first of the four reply-363 fixes. Sibling fixes: #1307 doc-fetch-visible-viewport, #1308 doc-fetch-since-prompt, #1309 doc-refresh-progress.
