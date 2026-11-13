---
title: 'discuss-mod8-toggle: Mod+8 toggles Discuss over the current view'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-12T20:10:00+00:00
updated_at: 2026-11-12T20:10:00+00:00
---

# Description

Owner replies 005/006 (via coordb): Mod+8 (Cmd-8) toggles the Discuss panel open/closed OVER the current view, restoring its last layout (overlay/docked/full) and transcript; NOT a view switch (unlike the other Mod+number keys). If Discuss is in full-screen state it simply looks like full-screen navigation. It must work from a message box too (like the everywhere Mod chords), and appear in the keymap and the shortcuts help.

# Design

- A new everywhere Mod+8 binding in the keymap (`createKeymap`, KEYS_VIEW beside Mod+0..4): heard at the window so it fires from a message box, the search, Settings or Help, and types nothing there. It is not a `ViewChoice` and never calls `chooseView`; the current view, screen and scroll stay exactly as they are.
- The act is `DiscussView.toggle()` (view/catchup): open when closed (the existing `open()` path — scope of the selection now, cache-only preview, no model call), close when open (the existing `close()`). While a catch-up is in flight or Discuss is unavailable it does nothing, as the bar button does. Reopening restores the kept layout (`discussMode`/`discussWidth`/`discussSheetFull`, per device) and the transcript: in-session from the panel DOM today, and from the server record once #1345 discuss-transcript-persist lands (its restore lives inside `openWith`, so the toggle needs no record call — seam agreed with mu-catchup).
- Scope binding (#1395, mu-discussbind) is respected by construction: the toggle only calls view open/close/isOpen, never panel internals; same-scope reopen restores the same answer, a different scope starts fresh through the view's existing scope sync/invalidate.
- Wiring follows the outline chord's shape: an optional `toggleDiscuss` act on the keyboard composition, handed by voice.js to `discussView.toggle()`; the shortcuts help reads the binding off the keymap, so listing and behaviour cannot drift.

# Acceptance Criteria

- Mod+8 opens Discuss over whatever view/screen is up without changing the view, and again closes it; it fires from a message box and the search as from anywhere else, and a held repeat does not flicker it.
- Reopening restores the last layout (overlay/docked/maximized, width, phone sheet height) and the last transcript (in-session; the #1345 record when that lands), spending no model call.
- The shortcuts help lists Mod+8 as the Discuss toggle; page tests pin the binding, the toggle and the no-view-switch behaviour; the Discuss/keyboard browser walks pass at phone and desktop sizes.

# Notes

Owner replies 005/006. Seams agreed with mu-catchup (#1345 restore inside openWith; toggle uses only view open/close/isOpen) and requested from mu-discussbind (#1395 scope binding owned there). Next Discuss batch with #1345, #1387, #1395.

## Resolution

[watermelon-full-preview] Delivered in the evening Discuss batch e7207edb46abd33c2c1a268c7609013f4c4f4c0a (replayed as 687dcd56 with the #1395 binding follow-up 956aa507; integrator-gated, deployed by coordb). Mod+8 toggles Discuss over the current view from anywhere including a message box, restoring the kept layout and transcript with no model call; the shortcuts help lists it from the same keymap. Focused JS tests, typecheck and the Discuss browser walk (including Control+8 close/reopen) passed before the batch gate.
