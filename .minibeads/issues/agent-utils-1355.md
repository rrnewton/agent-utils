---
title: 'phone-bar-discuss-slot: a fixed Discuss icon on the phone bar, audio in the reader row, Show/Fold in the hamburger menu'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-11T19:25:00+00:00
updated_at: 2026-11-12T13:20:00+00:00
closed_at: 2026-11-12T13:20:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 416 (2026-11-11): "On our space constrained mobile toolbar let's move the device/cloude audio control into the HIDABLE reader bar. Let's move Show/Fold read into the hamburger icon menu. Let's thereby make room for one fixed width icon replacing the cloud/device audio control but sitting to the right of the thread selector, just left of the hamburger icon."

The liaison's interpretation, given to the owner (who may correct it): the freed slot holds the Discuss icon (the Discuss side agent, #1316 catchup-side-agent, now opened from the hamburger action menu on a phone). Owner priority confirmed 2026-11-12 (reply 002): the fixed-size Discuss button must be visible on mobile.

Phone bar only; the desktop bar is unchanged except where it is trivially the same code path. Coordination: mu-discusslay (#1344/#1354 Discuss panel layout) and mu-catchup (Discuss panel behaviour) both confirmed no conflict with reusing `#discuss-open` and its exact click path; mu-readtoggle (#1168) rebases its read-toggle change onto this move.

# Design

- `#audio-source` (device/cloud read-aloud toggle, same id, switch state and icons) moves from `#bar-pack` into the hidable reader row `#control-pane`, in the narrow column Show/Fold vacated. `renderControlBar` states its `hidden` on the views it served on the bar (discord/docs, only with a configured agent); it hides with the reader row as the row does.
- `#todo-filter` (Show/Fold/Hide read, `#221 read-modes`) moves from `#control-pane` into the hamburger `#action-menu` as a tray button, keeping its id, `#todo-filter-label` verb/noun spans, cycle, label state, `call.ts` `hidden = !channel` logic and the `h` keyboard shortcut (which works with the menu closed, as it did with the reader row hidden). It joins `ACTION_MENU_ITEMS` and the menu-closing listener loop, so choosing it cycles and closes the menu. Behaviour is unchanged: #1168 lands on top of this relocation.
- `#discuss-open` stays the pack's last member with its existing click path (`panel.openWith`, the same act as `#discuss-menu-open`, which is kept). On a phone it is no longer `display: none`: `22-discuss.css` draws it as a fixed 2.75rem icon-only button right of `#thread-select` and just left of `#action-menu-open`. On a desk it keeps its labelled bar button and the icon is not drawn.
- No `web/voice.js` edit (#624 owns it), no scroll-position writes (#1251 owns scroll). The bar bargain is kept: the channel name gets more than twice the thread picker's width and there is no sideways scroll.

# Acceptance Criteria

- On a phone the bar shows a fixed-width (44px) Discuss icon right of the thread picker and just left of the hamburger; tapping it opens Discuss exactly as the menu item does, and the menu item remains.
- The device/cloud audio toggle is drawn in the hidable reader row, not in the bar pack, and keeps its device/cloud state and tap behaviour.
- Show/Fold/Hide read is offered in the hamburger menu, cycles and closes the menu, keeps its label state, and the `h` key still cycles it with the menu (and reader row) closed or hidden.
- Bar-fit tests cover 375/393/412px: the JS page suite prices the bar and asserts the new positions, and the Chromium command-bar walk measures the same widths with no sideways scroll; the channel name keeps more than twice the picker's width.
- Screenshots at 390x844 and 375px are captured for this issue.
- The change is gated at the owner tier with path selection and landed on main.

# Notes

Lanes: mu-phonebar (this issue), mu-discusslay (Discuss panel layout, #1344/#1354), mu-catchup (Discuss panel behaviour, #1346 pending state rebases over this), mu-readtoggle (#1168 read toggle rebases over the `#todo-filter` relocation). Filed from the owner's fast-track brief; number reserved by coordb.

## Resolution

[watermelon-full-preview] Delivered with this closing commit. On a phone `#discuss-open` is a fixed 44px icon (the pack's last member, right of the thread picker, just left of the hamburger, same click path and availability drawing as before; the menu item stays), `#audio-source` lives in the hidable reader row in the column Show/Fold vacated (shown only while the reader row is the pane's content), and `#todo-filter` is a tray item in the hamburger menu with its id, label spans, cycle and `h` key unchanged. Phone-only spacing (tighter bar/pack gaps, a 4.25rem switch word, the channel picker growing to the leftover) keeps the channel name over twice the picker's width with no sideways scroll; the desktop labelled Discuss button and bar are otherwise unchanged.

Evidence: `make -C vibe-talk typecheck` passes; the JS view suites for catchup/channel/dom pass; the page suite passes except one pre-existing environment failure (`#532 shift-o-latency` needs `process.threadCpuUsage`, absent from this host's Node 16, untouched by this change) and four view suites that import `.ts` directly fail the same way on this Node. The Chromium command-bar walk passes at 375/390/393/412px and on the desk, measuring Discuss at 44px, the channel at 89-126px and no sideways scroll, plus the menu's Show/Fold cycle-and-close. Screenshots: [390x844](https://pxl.cl/l5p5W) and [375x667](https://pxl.cl/l5p91). The Discuss browser walk's phone path now taps the bar icon; a fresh release build (its server binary) is exercised by the gate rather than locally. Gated at the owner tier with path selection and landed on main.
