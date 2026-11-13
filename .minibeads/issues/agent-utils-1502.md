---
title: 'discuss-button-pinned-regression: Discuss pinned to the toolbar at every width, never demoted to the menu'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-13T23:17:00+00:00
updated_at: 2026-11-13T23:17:00+00:00
closed_at: 2026-11-13T23:17:00+00:00
---

# Description

[watermelon-full-preview] Owner request 045 (2026-11-13): "I did just notice even though I saw the sparkle button a moment ago. I looked at my mobile app and the sparkle button was gone and it was demoted back into the hamburger menu. I thought we were going to have that fixed with sparkle icon for the discuss panel. As always available in the toolbar. Is it because I did hide and show on the reader and that changed something?"

Yes: `#1470 toolbar-dynamic-promotion` made Discuss the top-priority promotion candidate, so when Hide reader then Show reader shrank the bar's measured free room on a phone, the promotion controller demoted Discuss off the bar (`#discuss-open` hidden, `#discuss-menu-open` shown), undoing `#1355 phone-bar-discuss-slot`'s fixed Discuss icon. References: `#1355 phone-bar-discuss-slot`, `#1470 toolbar-dynamic-promotion`.

# Design

- Discuss is pinned, not promoted: it leaves `PROMOTION_ORDER` and stands on the bar at every width and in every reader state. `#discuss-menu-open` stays hidden while the bar has it, keeping `#1470`'s never-listed-twice invariant.
- Only Fold read, Expand/Collapse and Summaries promote by measured fit, and fit is measured with Discuss's slot already taken: `#discuss-open` counts as a fixed pack member in the free-width arithmetic.
- No fallback demotion: if the bar cannot fit Discuss beside its fixed members at the narrowest supported width, that is reported, never solved by demoting Discuss.

# Acceptance Criteria

- Discuss is on the toolbar at every width, phone included, whatever the reader state; it is never demoted and never listed in the menu while on the bar.
- Fold read, Expand/Collapse and Summaries still promote and demote by measured fit with hysteresis, priced after Discuss's slot.
- A module test proves the promotion controller never demotes Discuss at any overflow and that the menu hides `#discuss-menu-open` while `#discuss-open` is shown.
- A browser check at 360px and the usual phone viewport shows Discuss staying on the bar after Hide reader then Show reader, and after a resize, with condition waits only.

# Notes

Lane: mu-discusspin (this issue). Filed from the owner's fast-track brief (owner request 045); number reserved by coordb. Sibling Discuss lanes mu-discussread (`#1501`) and mu-discussstrike (`#1388`) touch neighbouring files; this change is confined to the promotion controller, its tests, the page-suite assertion and the command-bar browser walk.

## Resolution

[watermelon-full-preview] Delivered with this closing commit. `promotion.ts` no longer treats Discuss as a candidate: `PROMOTION_ORDER` is Fold read, Expand/Collapse, Summaries; the controller starts with nothing promoted, prices `#discuss-open` as a fixed pack member before any candidate is measured, always shows it where Discuss is offered, and keeps `#discuss-menu-open` hidden in every state. The page suite now requires Discuss on the channel bar with no menu listing.

Evidence: the promotion module suite passes 10/10, including a width sweep (900px down to 0 and back) that never demotes Discuss; the page shell suite passes 77/77 and the catchup view suite 38/38; `make -C vibe-talk typecheck` passes. The Chromium command-bar walk passes, including the new pinned check at 360x740 and 390x844: Discuss stayed on the bar through Hide reader, Show reader and a resize, never listed in the menu. Fit at the narrow widths, measured in that walk: at 375px Discuss is drawn at the fixed 44px with no sideways scroll in the bar or page; at 360px Discuss is still drawn at 44px and the page does not scroll sideways (the pack itself has 6px of internal overflow below the 375px width the bar is priced for, which predates this change and does not hide Discuss). Gated at the owner tier and landed on main.
