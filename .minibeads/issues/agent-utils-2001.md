---
title: 'vibe-talk-documents-toolbar-star-downloaded: star and downloaded filters beside the hamburger on Documents'
status: closed
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
- Both toggles compose with each other and with the All/Markdown/Pastes/Issues chips. Both are page-session state exactly as the star filter is today: a reload returns the full list, and what survives a reload is the per-document state itself (a document's star and its fetched copy), as the existing browser walk asserts.
- No list scroll behaviour changes. Phone 393/412 and desktop 1280 must fit without wrapping or overflowing.

# Acceptance Criteria

- Page tests cover placement order, both filters alone and combined with the chips, and the reload behaviour the star filter has today (session toggles, persistent per-document state).
- Phone 393/412 and desktop 1280 toolbars fit without wrapping or overflowing, in the page fit pricing and in browser screenshots.
- Screenshot matrix (light and dark, phone 412 and desktop 1280, keyboard closed, plus the toolbar with each filter on), with BUILD.txt in every folder.

# Notes

Owner screenshot (phone) showed the star at the left of the toolbar, away from the thumb-side hamburger. The downloaded filter was requested with the download icon in the same breath.

## Resolution

[watermelon-full-preview] Landed 132433df on github/main (gated tree ec5adcf1, replayed over 93cf0b15). The Downloaded/Starred pair stands left of the hamburger, both filters compose with the bucket chips, page tests and the documents toolbar/documents/download-jump/fresh-load browser walks pass, and the screenshot matrix (dark/light, phone 412, desktop 1280, each filter on) was reviewed.

## Follow-up

Phone card meta (pre-existing on 93cf0b15, not introduced here, verified with the same seeded list on untouched main): on a 412px phone the file-name part flex-shrinks to zero width, so the meta reads "· mentioned …" with a leading separator, and unfetched rows cut the mentions part mid-word ("·1 menti"). The DOM join is correct (`#1994`); the defect is the `.doc-meta` flex sizing — the file name should keep a floor or drop with its separator, and the tail should ellipsise cleanly. Filed here as a follow-up for the documents meta owner.
