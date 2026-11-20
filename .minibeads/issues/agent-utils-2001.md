---
title: 'vibe-talk-documents-toolbar-star-downloaded: star and downloaded filters beside the hamburger on Documents'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-20T16:54:00.000000000+00:00
updated_at: 2026-11-20T16:54:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner UI request (271): "The star button for filtering on the toolbar is not where I wanted it to go on the documents view. I wanted it just to the left of the hamburger icon on the right hand side for easier reachability. I also want the download icon right next to it to filter for downloaded documents." The phone screenshot shows the Documents toolbar as [gear][star][cloud] ... [hamburger][Fetch][Documents], with the star at the left. Related: #1909 vibe-talk-documents-filter-bar-fixed (where Starred became a pack toggle), #1265 documents-star-hide, #1176 doc-kind-buckets.

# Design

- On the Documents list the two filter toggles stand as a pair immediately left of the hamburger, outside the scrolling pack, in the order [downloaded][star][hamburger][Fetch][Documents]. The star stays immediately left of the hamburger because the owner named that adjacency; the downloaded toggle sits left of the star rather than between the star and the hamburger, so that adjacency is literal.
- Downloaded means a fetched local copy (`row.fetched`, the same state the row's download button shows as fetched). The toggle uses the row's download glyph, with `aria-label` and `aria-pressed`; the star keeps `aria-label`/pressed and its outline/filled glyphs. Both are native buttons, so keyboard operation follows.
- Both toggles compose with each other and with the All/Markdown/Pastes/Issues chips. Both persist across a reload in device storage (the star filter is in-memory today, so this also gives it the reload survival the owner expects).
- No list scroll behaviour changes. Phone 393/412 and desktop 1280 must fit without wrapping or overflowing.

# Acceptance Criteria

- Page tests cover placement order, both filters alone and combined with the chips, and persistence across a reload.
- Phone 393/412 and desktop 1280 toolbars fit without wrapping or overflowing, in the page fit pricing and in browser screenshots.
- Screenshot matrix (light and dark, phone 412 and desktop 1280, keyboard closed, plus the toolbar with each filter on), with BUILD.txt in every folder.

# Notes

Owner screenshot (phone) showed the star at the left of the toolbar, away from the thumb-side hamburger. The downloaded filter was requested with the download icon in the same breath.
