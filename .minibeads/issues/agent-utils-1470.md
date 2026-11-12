---
title: 'toolbar-dynamic-promotion: promote menu actions to the toolbar by measured fit, never listed twice'
status: closed
priority: 1
issue_type: task
created_at: 2026-11-12T20:32:00.000000000+00:00
updated_at: 2026-11-12T20:32:00.000000000+00:00
---

# Summary

Owner request (Slack 030, 2026-11-12): Discuss is listed twice, in the
hamburger menu and on the toolbar. No action should be listed twice.
On desktop the dock has free room between the Discuss button and the
agent/device audio button; promote actions to the toolbar/dock
opportunistically, in priority order: Discuss, Fold read,
Expand all / Collapse all, Summaries. Promote as many as fit in the
measured free space; the rest stay in the menu. The phone gets the
same rule, which with little room means mostly the menu.

This lane stacks on the read-toggle lane (#1168 binary Fold read,
#1399 fixed labels and shared active style): it consumes that lane's
fixed labels and `aria-pressed` active style and does not redo them.

# Acceptance

- No item is listed twice in any state: an action on the toolbar is
  not in the hamburger menu, and an action in the menu is not on the
  toolbar. Discuss keeps both its ids (`#discuss-open`,
  `#discuss-menu-open`) as one action with exactly one visible.
- Promotion is dynamic, in the priority order Discuss, Fold read,
  Expand all / Collapse all (one slot), Summaries: as many as fit in
  the measured free space are promoted, the rest stay in the menu.
- Fit is re-measured on resize and on layout changes (dock/undock
  Discuss, one/two columns) with a ResizeObserver on the toolbar or
  measured free space, not viewport breakpoints.
- Hysteresis keeps items from flickering at the fit boundary.
- Promoted toggles use the read-toggle lane's fixed labels and shared
  active style (blue outline plus blue text, via `aria-pressed`). A
  promoted item and its menu form are the same action with the same
  state and one implementation: single elements move between menu
  and toolbar, never clones.
- Page unit tests cover the promotion decision fed measured widths
  (all fit, some fit, none fit), no duplicates in any state, and
  promoted state staying in sync with the menu form.
- Browser walks at 1280x800, 1920x1080 and phone width, with
  screenshots. No wall-clock waits.

# Design

One new module, `vibe-talk/web/src/view/shell/promotion.ts`:

- A pure decision function: given the free width, each candidate's
  measured width, and the currently promoted set, return the new
  promoted set. Candidates in priority order; promote the next only
  when it fits with hysteresis slack to spare, demote the lowest
  priority only when the promoted total overflows by more than the
  slack, so a boundary width holds its state instead of flickering.
- A controller that measures the bar pack's free space (pack width
  minus its non-candidate members and gaps), measures candidate
  widths (probing synchronously with visibility hidden when a
  candidate sits in the closed menu, where it has no layout width),
  applies the decision, and re-runs from a ResizeObserver on the
  control bar plus an explicit refresh from `renderControlBar`,
  which already runs on every layout change.
- Applying a decision moves single elements: Fold read
  (`#todo-filter`), Expand all / Collapse all (`#expand-all`,
  `#collapse-all`, one slot) and Summaries (`#summarise`) move
  between `#action-menu` and `#bar-pack`; Discuss, which has an
  element in each place by design, shows exactly one of its pair.
  Availability (`hidden`, owned by the existing per-view renderers)
  is never written for moved items, so state and handlers are the
  same element in either home.

Wiring is minimal and stack-friendly: `createControlBar` refreshes
promotion at the end of `renderControlBar`, the action menu's empty
line counts only items actually in the menu, and promoted items get
bar sizing in the control-bar stylesheet. No label or active-style
work: that is the read-toggle lane's, consumed via `aria-pressed`.

# Notes

Stacked on the read-toggle lane's branch; rebased onto its head as
it moves, then onto main once it lands. Batch: no gate alone; the
coordinator names the integrator when this lane reports its branch,
head and base.

## Resolution

Delivered on branch `mu-toolbarpromo/1470`, stacked on
`mu-readtoggle/read-toggle-two-way` (its work was still uncommitted
at delivery, so the integrator rebases this branch onto that lane's
commits, then onto main). New `promotion.ts` (pure measured-fit
decision with hysteresis, ResizeObserver on the control bar, refresh
from `renderControlBar`) moves single elements between the menu and
the bar in the priority Discuss, Fold read, Expand/Collapse,
Summaries; Discuss shows exactly one of its kept pair. Unit tests
cover all-fit / some-fit / none-fit decisions, hysteresis, the
no-duplicates invariant and state sync; browser walks at 1280x800,
1920x1080 and phone width passed with screenshots. The full page
suite shows no new failures (five suites fail identically on the
base in this environment). Unverified until the batch lands: the
read-toggle lane's shared pressed style on promoted toggles (its
commit), and rebase onto its final head.
