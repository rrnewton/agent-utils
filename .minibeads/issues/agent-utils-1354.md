---
title: 'discuss-layout-modes: overlay, docked, and maximized panel layouts'
status: closed
priority: 0
issue_type: feature
assignee: mu-discusslay
labels:
- vibe-talk
created_at: 2026-11-11T19:02:00+00:00
updated_at: 2026-11-12T14:00:00+00:00
---

# Description

Owner reply 415 (2026-11-11): "I think we could continue the overlay model (with adjustable width)... but we might want the option to dock it too for multi-column side-by-side OR to maximize it to just talk to the Discuss helper agent."

Lane mu-discusslay owns the Discuss panel's LAYOUT. This issue follows `#1344 discuss-panel-resize` (wider resizable overlay, phone half-height sheet, landed first).

# Design

- **Three modes.** Overlay (the resizable overlay of #1344), docked (a side-by-side column), maximized (fills the main area to just talk to Discuss). The choice is `DEVICE_SETTINGS.discussMode` (`vibe-talk.voice.discuss-mode`), per device as the width is, overlay for anything unknown or refused.
- **Docked reuses the desk's two-column machinery** (`#229 desktop-two-column`): the panel takes the side column's grid place in `#screen-main` (column 2, every row; a third column when two columns already stand), sized by the same kept `--discuss-width`. The panel only names the mode (`data-mode` on it, `data-discuss` on the screen); the stylesheet lays it out. No new layout system, and no scroll position is ever written: the scroll owner's stability hold (`#1251`) keeps the reader's place across the one reflow.
- **Mode control.** Three small buttons (Overlay/Dock/Full, `aria-pressed`) in the header's `#discuss-actions` slot beside Close. On a phone the control is hidden: the sheet and its Maximize stand as #1344 left them, and the stylesheet gives docked no column there.
- **Keys and focus, as the other panels.** Opening puts focus on Close (`preventScroll`); Escape closes and returns focus to the opener. Construction stays inert; every layout dep is read on use.

# Acceptance Criteria

- Desktop 1280x800: the mode control switches overlay/docked/maximized; docked stands as a column beside the list at the kept width (a third column beside two columns); maximized fills the main area; the mode persists per device across close/reopen; opening and mode switches never move `#scroll-area` by a written scroll position.
- Phone 390x844 and 393x852: no mode control; the full-width half/full sheet of #1344 is unchanged.
- Unit tests pin the mode default, keeping, and focus/Escape; the Discuss browser walk asserts all modes in Chromium; the owner-tier gate passes.

# Notes

Owner replies 415 (dock/maximize), 413/417 (#1344, landed as 15a8a857, live 2026-11-12). Built on #1344's `#discuss-actions` slot and device settings; mu-catchup's #1346 Summarize joins the same slot. Landed by mu-discusslay; coordb deploys.
