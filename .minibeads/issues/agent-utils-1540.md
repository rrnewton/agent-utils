---
title: 'discuss-footer-inline-links: Discuss footer times as one inline comma-separated list'
status: closed
priority: 1
issue_type: bug
assignee: mu-footerlinks
labels:
- vibe-talk
created_at: 2026-11-14T18:30:00.000000000+00:00
updated_at: 2026-11-14T18:30:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner UI requests 064+065 (2026-11-14, phone screenshots of the Discuss summary): the summary's "Other messages processed:" footer still shows one full-width button per time (13:44, 13:46, 14:01, ...) and fills the screen. The owner has asked several times for a dense list on one line, "13:44, 13:46, ...", each time a link. Diagnosis: #1347 shipped the footer content (only the uncited remainder, HH:MM labels) but `drawFooter()` in `vibe-talk/web/src/view/catchup/panel.ts` still built an `<li><button class="discuss-link">` per time and `vibe-talk/web/styles/voice/22-discuss.css` still laid `#discuss-links` out as a column with `.discuss-link` as a full-width block, so each time rendered as a full-width row. The acceptance test checked the text, not the layout, so the regression was invisible to it.

# Design

- Footer only, in `panel.ts` `drawFooter()` and `22-discuss.css`: the uncited remainder renders as inline, comma-separated links that wrap like text — "Other messages processed: 13:44, 13:46, 14:01, ..." — with the label and the times in one flow at phone and desktop widths.
- Each time keeps the same jump handler (`onJump` with the same ref), stays a real `<button>` (keyboard focusable, visible focus outline unchanged), and gets a padded inline-block tap target (at least 32px tall in the browser walk) without becoming a block.
- No changes to `voice.js`, `main.js`, the contract, or the sibling Documents/Discuss areas (#1541–#1545 land on their own).

# Acceptance Criteria

- At 390px and desktop, many footer times fit in at most 3 lines, links share lines, no link stretches full-width, the times are comma-separated in the footer text, and the label shares the times' flow.
- Clicking a footer time still jumps with the panel staying open, as before.
- A browser test measures the layout (bounding-box line sharing and width), not just the text, at phone (390/393px) and desktop widths, with condition waits only.

# Notes

Owner UI fast-track (owner rule 286), requests 064+065. Sibling lanes mu-docnav (#1541), mu-outlinelist (#1542), mu-readerclick (#1543), mu-docbar (#1544) and mu-hamburger (#1545) land on their own; whoever lands second rebases and re-gates. Gate: owner tier (GATE_PRIORITY=owner), then land-main; coordb deploys. Why it was missed, in one line for the owner: #1347's acceptance checked what the footer said, never how it was laid out, so a full-width button per time passed a text-only test.

## Resolution

The Discuss footer now reads as one dense line — "Other messages processed:" followed by its uncited times as inline comma-separated links that wrap like text — instead of one full-width button per time filling the screen. Each time is still its own jump button with the same handler, keyboard-focusable with a padded tap target. The Discuss browser walk now measures the drawn layout at 390px, 393px and desktop (line count, line sharing, link width, tap height, comma separation) so a text-correct but screen-filling footer fails the test.
