---
title: 'search-whose-disc-hangs-alone-at-360: on a phone up to 400px wide the Whose button hangs alone under the open search bar, detached from its row, and a standing search covers it'
status: open
priority: 3
issue_type: bug
assignee: cl-datehint
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-28T15:02:00-05:00
updated_at: 2026-11-28T15:02:00-05:00
---

# Description

Found by the advisory design review of `#2490` search-date-hint-beside-empty-pins (rule R2, narrow button groups). The liaison accepted it as out of scope for `#2490` and asked for its own issue.

On a phone up to 400px wide (`#251 author-filter`, 09-to-do-view.css `@media (max-width: 400px)`), the open search bar's tool row would leave the field too few letters, so the Whose button (author filter) hangs UNDER the bar's left end as a lone disc on the bar's panel. The list starts below it (`--hang-foot`).

The stills (light, 360x800, the page live since `#2425`; kept in the `#2490` lane's evidence and sent to the coordinator) show three things:

- With the bar open and nothing typed, Whose stands as an orphan circle below the bar. It belongs to no row. The calendar stands beside the field on the first row; Whole threads, Links, Labels, the glass and close stand on the second.
- Anything placed under the bar has to clear the disc. The `#2425` date hint stood 51 CSS px below the bar's foot, detached from the field. With `#2490` (in flight) the hint becomes a caption in the bar's sheet, and it steps aside at this width.
- With a search typed, the bucket chips' panel draws over the disc, and Whose is not visible at all while the chips stand.

R2: on a narrow width a page-level button group lays out as equal-width columns filling the content width, or as full-width stacked rows. A single control hanging alone outside its group does neither.

# Design

- First, measure at 360 and 400px whether the bar's second row can hold Whose again. The hang dates from the one-line bar (`#251`: a third square took the field under 100px at 360). Since `#1852` the phone bar has two rows, and that reason may no longer hold. The second row would hold six targets (Whole threads, Whose, Links, Labels, glass, close): 6 x 48 + 5 x 8 = 328px (R9), against a bar about 344px wide at 360.
- Options to compare with stills:
  - (a) Whose rejoins the second row, the row's targets taking equal-width columns (R2), with close kept at the end. This only works if it fits R9 at 360.
  - (b) A third full-width row inside the bar's sheet for the overflow toggles. The bar grows; the list's head room grows with it, constant whatever is focused or typed. Nothing may move with focus: the `#1852` rule, under which no tap can move its own target.
  - (c) Keep the hang but give it a home: a row of its own inside the bar's sheet, never a lone disc over the list.
- Whatever the choice: the bucket chips and the `#2490` date caption never cover a toggle, and the caption can stand at this width again.
- No change to the field's minimum width (`#211 link-filter`) without the liaison.

# Acceptance Criteria

- At 360 and 400px, light and dark, with the bar open: Whose stands inside the bar's sheet, in a row with the other toggles or a row of its own, never alone over the list. With a search typed, no chip covers it.
- Every toggle remains a 48x48 target with 8px between targets (R9); a walk asserts each toggle's centre hits the toggle, in every state (open, typed, bucket chips standing, date caption standing).
- The `#2490` date caption stands at 360-400px again, as part of the bar's sheet.
- Screenshot matrix before and after (R16), with the advisory design review.

# Notes

- Filed by cl-datehint at the coordinator's request (number pre-assigned), from the `#2490` design review (finding R2, accepted as a separate issue by the liaison).
- Related: `#251 author-filter` (the hang), `#1852` / `#1853` (the phone bar's two rows), `#2425` / `#2490` (the date hint), `#2200` (the bucket chips' head room).
