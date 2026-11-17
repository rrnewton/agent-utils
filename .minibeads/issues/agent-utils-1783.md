---
title: 'vibe-talk-review-nits-1761-1762-1767: hours box, Newest chip overlap, Documents count follow-ups'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- ui
created_at: 2026-11-17T12:12:16-05:00
updated_at: 2026-11-17T12:12:16-05:00
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
