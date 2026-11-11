---
title: 'doc-back-side-thread-hidden: Back from a side-column document restores the side thread'
status: open
priority: 0
issue_type: bug
assignee: mu-docread2
labels:
- vibe-talk
- documents
created_at: 2026-11-11T18:15:24+00:00
updated_at: 2026-11-11T18:15:24+00:00
---

# Description

[watermelon-full-preview] Found as a side observation during the #1326 doc-link-bare-url investigation (agent probe, real Chromium): after Back from a document opened from the side column, the side thread container stays display:none in the probe, so the thread the reader came from is not visible again. Other Back paths restore their origin; this one leaves the side column hidden.

The coordinator assigned this number from the devbig014 block and directed a fix as its own small landing at owner tier, with a browser test: open a document from the side column, press Back, and the side thread is visible again.

# Design

- Reproduce first in the browser harness (two-column desk layout): open a document from a side-column thread message, press the reader's Back, and observe the side container's visibility and content. Trace the show/hide chain (documents view open/back, shell column containers, reader onBack wiring) and fix the proven broken step with the smallest change.
- No vibe-talk/web/voice.js edit, and no vibe-talk/web/src/view/documents/index.ts or list.ts edit, without first naming the exact functions to the coordinator (another lane works in the documents list/index; rebase over its landings if they move first).
- Browser test with real input and condition waits only, no wall-clock budgets (owner rule 346): desk side-column open, Back, side thread visible again; phone included where the same container path applies. Owner fast-track: gate at owner tier with path selection and land alone; the coordinator deploys on landing.

# Acceptance Criteria

- The new browser test fails before the fix (side thread hidden after Back) and passes after it (side thread visible with its content), at 1280x800 and, where the same path applies, 393x852 with touch.
- Existing documents and side-column checks stay green; Back from main-column and list documents is unchanged.
- The fix is gated at owner tier and landed on its own.

# Notes

Coordinator-assigned from the devbig014 ledger block after the #1326 doc-link-bare-url probe observation. Filed by mu-docread2 before implementation; the fix lands alone as landing D. Related: #1326 doc-link-bare-url (the investigation that surfaced this), #1327 doc-reader-kind-header, #1328 doc-heading-size, #1330 doc-read-highlight.
