---
title: 'discuss-header-icons: no title row; dock, fullscreen and close as three small square icons'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-12T17:10:00+00:00
updated_at: 2026-11-12T17:10:00+00:00
---

# Description

Owner replies 005/006 (via coordb): "I don't think we need the word 'Discuss' sitting in the first row looking awkward". The panel's first row is an h2 "Discuss" title plus a row of text buttons (Overlay/Dock/Full, Summarize, Maximize, Close from #1344/#1354/#1346), costing a full row of vertical space before any content.

# Design

- No title row. The panel's control row (the #1382 one-line Summarize control) carries, at its upper right, three small square icon buttons that consume no extra vertical space: a dock toggle (overlay <-> docked, a dock icon), a fullscreen toggle (the standard arrows-apart icon; maximized on a desk, full/half sheet on a phone), and close (X, #discuss-close, id unchanged).
- The three text mode buttons (#discuss-mode-overlay/docked/maximized) and the sheet text toggle (#discuss-sheet-toggle) collapse into the two toggles; the kept per-device choices are unchanged (`DEVICE_SETTINGS.discussMode`, `discussSheetFull`), so reopening restores the last layout exactly as #1344/#1355 left it. Each toggle names its state for assistive tech (aria-label/aria-pressed) and on hover (title).
- Focus on open still lands on Close; Escape still closes and returns focus. No scroll position is written (#1251).

# Acceptance Criteria

- The panel has no "Discuss" title row; its first row is the Summarize control line with three small square icons at the upper right (dock, fullscreen, X) at 390x844, 375x667 and 1280x800.
- The dock toggle switches overlay/docked and the fullscreen toggle switches maximized (desk) or full/half sheet (phone); both choices persist per device across close/reopen; Close and Escape behave as before.
- Page tests pin the toggles, keeping and focus/Escape; the Discuss browser walk asserts all three icons in Chromium; the owner-tier gate passes.

# Notes

Owner replies 005/006; shape confirmed by coordb (exactly the owner's three icons). Lands with #1382 discuss-summarize-control-oneline as one owner-tier landing. mu-catchup confirmed #discuss-close is the only header id its tests bind.

## Resolution

[watermelon-full-preview] Delivered in e5fc567c (4:30 PM ET Discuss small-fix batch, owner reply 012; integrator mu-discussfoot, gated 851343ad). The Discuss title row is gone; dock, fullscreen and close stand as three small square icons at the upper right of the control row, with the kept per-device layout choices unchanged. Focused tests and the Discuss browser walk passed in the batch gate.
