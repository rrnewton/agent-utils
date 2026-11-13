---
title: 'discuss-fold-state-persist: keep Discuss section folds across close/reopen and reload'
status: closed
priority: 1
issue_type: feature
assignee: mu-discussfold
labels:
- vibe-talk
created_at: 2026-11-13T20:45:33.097112871+00:00
updated_at: 2026-11-13T21:20:21.885350702+00:00
closed_at: 2026-11-13T21:20:04.410703928+00:00
---

# Description

[watermelon-full-preview] Owner request 042 (11-13 3:40 PM ET), verbatim: "When I close/open Discuss let's save its expanded/collapsed state for the summary. I don't want it to reset."

#1387 (e0b9cb7f) folds the Discuss answer sections, all collapsed, with Expand/Collapse all. #1345 (addec444) saves the Discuss record server-side. Today the fold state resets every time Discuss reopens.

# Design

- Keep it on the page: store the open sections in the page's existing device-settings storage (DEVICE_SETTINGS in web/src/core/settings/device.ts), following the discussWidth/discussSheetFull/discussMode pattern through panel deps (storedFolds/keepFolds), catchup/index.ts forwarding, and voice.js wiring.
- One storage slot only, key vibe-talk.voice.discuss-folds, holding JSON {record, open[]}: the record identity is the scope (channel + thread, as the server keys DiscussRecord) plus the saved summary's created_ms generation, which asCatchupRecord already reads. Each keep overwrites the single slot, so a new summary drops the old entry; no growing pile of keys. Device-local, not in SYNCED_SETTING_KEYS.
- Stable per-section key from the summary structure: normalized heading text (duplicate headings disambiguated by occurrence), computed in web/src/view/dom/collapse.ts. foldSectionsByHeading takes the saved open set; openSectionKeys/foldSectionKeys read the state back; Expand/Collapse all goes through the same keep.
- The panel tracks the drawn summary's record key: set from the #1345 record draw, reused by an in-session restoreAnswer for the same scope, null for a fresh Summarize draw (all collapsed) until the saved record lands. A stored entry whose record does not match the drawn summary is ignored (stale/unknown keys have no effect).
- No server or contract change: the page already has the scope and created_ms.

# Acceptance Criteria

- Each section's expanded/collapsed state, and the Expand/Collapse all result, survives closing and reopening Discuss and a page reload.
- A new Summarize replaces the summary: the fresh summary starts all collapsed and the old entry is forgotten.
- Module tests cover restore, reset on a new summary, Expand/Collapse all persisting, and an unknown or stale key being ignored. The discuss browser check covers the folds with condition waits only (owner rule 346).

# Notes

Owner UI fast-track (owner rule 286), request 042. Lane mu-discussfold from coordb. Folds from #1387 discuss-collapsible-sections; saved record from #1345 discuss-transcript-persist. Gate: owner tier (GATE_PRIORITY=owner), then land-main and coordb deploys at once.

## Resolution

Kept each Discuss summary's open sections on the device (one vibe-talk.voice.discuss-folds slot holding the saved record — scope plus created_ms generation — and the open section keys), restored on close/reopen and reload, forgotten when a new Summarize replaces the summary. Module tests (collapse, device settings, panel) and the discuss browser walk cover restore, reset, Expand/Collapse all, and stale entries.
