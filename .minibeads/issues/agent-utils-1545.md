---
title: 'hamburger-show-hidden-list-only: Show hidden only in the Documents list; drop the empty Reader box'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-14T18:31:00.000000000+00:00
updated_at: 2026-11-14T18:31:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner UI request 066 item 9: in the hamburger menu, "Show hidden" appears only in the Documents list view (not the reader or other views); remove the empty vestigial "(Reader)" group. Check every view's hamburger for other empty groups and report them.

# Design

- The control bar decides the hamburger's items per view (`createControlBar`). "Show hidden" (`#1265`) was offered on the whole Documents view; it now also requires the list to be up — the reader (`#docs-reader` shown) has no list to unhide into. The Documents view itself keeps the same fact in step as it opens and closes documents, so the item changes with the reader even between bar redraws.
- The "(Reader)" box in the menu holds only Hide reader and Autoplay, both channel-only. Off the channel the box stood empty; the bar now hides the box itself wherever its toggles are not offered, and the box carries an id (`reader-box`) so it can be decided like its members.
- Audit of every view's hamburger for other empty groups: the menu's only grouped section is the Reader box. The call view, channel view, Documents list and Documents reader were each checked; no other group exists in the menu, and no other empty group was found. The remaining items are single buttons, each already shown only where it has work to do, with the menu's "Nothing to expand, collapse or summarize here." line standing when none is offered.

# Acceptance Criteria

- On the Documents list, the hamburger offers "Show hidden"; with a document open in the reader it does not, and returning to the list offers it again.
- No view's hamburger shows the empty Reader box; on the channel view the box still stands with Hide reader and Autoplay.
- A browser test at phone (~390 px) and desktop widths, plus unit and page tests, cover the visible behaviour.

# Notes

Owner UI request 066 item 9 (2026-11-14). Sibling lanes on the same page: #1540, #1541, #1542, #1543, #1544.
