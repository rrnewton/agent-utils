---
title: 'menu-above-chips: a message menu and every row popover draw above the floating chips'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-10T17:31:33+00:00
updated_at: 2026-11-10T21:01:59+00:00
closed_at: 2026-11-10T21:01:59+00:00
---

# Description

[watermelon-full-preview] Owner report (reply 324, 2026-11-10 11:35 AM ET): the message drop down is still below the floating chips. On desktop the owner clicks the ellipsis on a row and goes down to open the message in its own service, but the floating chips are higher in stacking order and obscure the menu being clicked. The owner notes this was pointed out a long time ago.

A message's ellipsis menu, and any popover or menu opened from a row (reactions, read popover, action menu and similar), must always draw above the floating chips, the read strip, the freshness pill and the composer, at desktop and phone sizes, wherever the row sits in the channel. A row near the bottom, close to the chips row, is the failing case the owner hit.

# Design

Make one stacking scale for overlays in the page (named CSS custom properties or one documented table) with the floating chips, the read strip, the freshness pill and the composer below it, and menus and popovers on top. Remove ad-hoc z-index values where they fight that scale.

The menu is built inside the row (`channelrows.ts`, `rowmenu.ts`): `row-more-menu` is absolutely positioned inside the row's meta line and carries `z-index: 5` in `09-to-do-view.css`, while the chips container and the strip and pill carry their own positional layers and z-index values across several stylesheets, with no shared scale. The menu can therefore be trapped below a sibling by a stacking context created by a `transform`, `filter`, or `position: fixed` parent, or by an inline parent that establishes a context before the menu's z-index is considered. The fix must not just raise a number but make the menu render into a top-level layer when a parent would trap it: check how the menu is mounted today, and where the chips, read strip, freshness pill and composer are mounted. The menu module is the owner for placement; CSS owns the z-index scale.

Browser test at desktop and phone sizes: open the ellipsis on a row near the bottom chips row and assert every menu item is the topmost element at its own centre (`document.elementFromPoint` returns the item or a descendant). Do the same for any other row popover (reactions, read popover) that can overlap the chips.

# Acceptance Criteria

- A browser test at desktop size and at phone size (393x852) opens the ellipsis on a row near the bottom chips row and asserts every menu item is the topmost element at its own centre (`document.elementFromPoint` returns the item or a descendant), including the item that opens the message in its own service.
- The same topmost assertion holds for other row popovers that can overlap the chips (reactions and the read popover where the page draws them over the list).
- One documented overlay stacking scale governs the chips, the read strip, the freshness pill, the composer, the ellipsis menu and other row popovers, with ad-hoc z-index values that fought it removed or folded into that scale.
- No regression: the floating chips still float over the list bottom, the read strip still floats over the chips while a read plays, and existing page tests for the menu, chips and strip keep passing.

# Notes

Owner reply 324, 2026-11-10. Prior-report search before filing:

- `.minibeads/issues` searched for `z-index`, `obscure`, `menu`, `chip`, `popover` and `stacking`. `z-index` hits only issue 819 (quoted-reply button always pressable, checking z-index and pointer-events against floating layers including chips); `obscure` hits nothing; `popover` hits only the search shortcut and the pace stepper; `stacking` hits unrelated lane items. No earlier issue names a message ellipsis menu drawn below the floating chips, and none names the chips obscuring a row menu. The nearest neighbouring issues are 819 (the quoted-reply button covered by a floating layer while the reader was paused) and 820 (the hamburger action menu that moves floating chips into a menu), which share the floating-layer ground but not this failure.
- `agent-utils/ai_docs` and the harness repo's `ai_docs` searched for the same terms. Only the channel-bars layout doc mentions the floating chips (as room the list keeps at its foot); no doc records the menu-below-chips failure.
- The vibe-talk backlog (`~/agent_scratch/vt-backlog.md`, 56 lines) searched for menu, chip, overlay and stacking. Nothing there names this failure either; the only stacking hits are unrelated queue work.

The earlier report therefore appears to have been made in chat and never filed as a tracked issue, so it had no number and no owner and was lost between page iterations. This issue files it now with an owner and a browser test that asserts topmost-ness directly, so a future chip change cannot silently cover the menu again.

Coordination: this is not scroll code, so there is no wait on 1210. Lanes for 1210 and 1209 edit channel rows and keepers, and the mention-jump lane for owner reply 312 edits the jump path. Keep edits in CSS and the menu module where possible; rebase onto moved code if the touched lines have moved.

## Resolution

[watermelon-full-preview] Landed on github/main as a8707fe5 (ea1f1b8f and a8707fe5), deployed by the coordinator on the owner UI fast-track. One overlay scale in :root (chips, strip, pill, menu, modal, debug) governs every floating layer; the row ellipsis menu renders in the top layer while open and stays where it opened. The jump-to-unread chip landed meanwhile inside the chips container and stacks on the chips layer below the menu layer, as agreed. Browser walk menu-above-chips at 1280x800 and 393x852 asserts every menu item topmost at its centre over the chips row; documents-browser 5/5 solo passes (load cause: three concurrent browser gates).
