---
title: 'vibe-talk-review-nits-1761-1762-1767: hours box, Newest chip overlap, Documents count follow-ups'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- ui
created_at: 2026-11-17T12:12:16-05:00
updated_at: 2026-11-18T05:12:06-05:00
closed_at: 2026-11-18T05:12:06-05:00
---

# Description

[watermelon-full-preview] Liaison review of the deployed batch (2026-11-17), three small follow-ups:

a) #1761 Discuss cap: the hours box clips values with 4+ digits. Size it to fit at least 5 digits, or cap its input sensibly.
b) #1762 search: the "Newest" chip overlaps the first expanded search hit. Reserve space for it, or move it.
c) #1767 Documents filter: the count line ignores the active filter. Show "N of M documents" (or similar) when a filter is active.

Reference shots of the deployed build: /tmp/mu-discusscap/shots, /tmp/mu-serverhits-shots, /tmp/docfilter-shots.

# Acceptance Criteria

- a) The Discuss hours input shows at least 5 digits in full at phone (412x915) and desktop (1280x800), light and dark, without clipping; the count input and the rest of the bound row are unchanged in behaviour.
- b) With server hits expanded, the Newest chip's bounding box does not intersect the first expanded server hit (or the expanded section's first control row); the hit remains readable and hittable. Reserve space or move the chip; collapsed behaviour unchanged.
- c) When any Documents filter is active (local name/title filter, bucket, or Starred), the count line shows the shown count against the total (for example "N of M documents"); with no filter active it reads as before.
- Page tests at phone and desktop sizes cover each exact defect, in light and dark where theme-related and keyboard open where layout-related, with condition-based waits only.
- Screenshot matrix on a TEST instance (light/dark x keyboard open/closed x phone/desktop, Discuss half-pane open in at least the phone shots, before and after each targeted state), every image inspected.

# Notes

[watermelon-full-preview] Filed by mu-uinits from the liaison's review of the deployed batch, bundled with #1779 vibe-talk-toast-avoids-discuss-controls in one lane at normal priority.

## Resolution

[watermelon-full-preview] Landed 40f359e4 on github/main (gate #8 full pass on 6ac6010b). (a) The hours clipping is fixed on the final base by #1813's follow-up (main, live): min-width: 8ch on #discuss-hours; this lane's two width hunks were dropped in the last rebase resolution, and the browser walk asserts a 12345 value's text fits the box, shot in /tmp/mu-uinits-shots/after5. (b) Expanded server hits (now #1786/#1797 timeline cards marked data-server-hit, and .server-hit quotes) carry scroll-margin-bottom of the measured chips clearance with a 2.75rem floor, so the first hit brought to the foot rests clear of the Newest chip; the overscroll trim keeps that reserve off #server-more/#search-further after search_unmatched_browser bisected their press timing to it. Both search suites pass. (c) The Documents head now reads "N of M documents" whenever visible cards narrow below the loaded documents (name, bucket, or Starred) and the old text otherwise; documents.test.mjs asserts 2 of 3, 1 of 3, and the plain line after clearing. Screenshot matrix /tmp/mu-uinits-shots/after5 covers the page states (the Documents count itself is DOM-proven by the node suite; no lightweight browser fixture serves /api/v1/documents). Liaison review pending before announcement.
