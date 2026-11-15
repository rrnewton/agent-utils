---
title: 'quote-reply-key: q for quoted reply on a selection'
status: closed
priority: 1
issue_type: task
external_ref: gh-311
created_at: 2026-11-08T00:19:17.835224052+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
closed_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

GitHub issue #311 quote-reply-key tracks this for outside readers.

## Description

Owner (reply 208): with text selected in a message, pressing q should do what the quote-reply button does, instead of hunting the button with the mouse.

## Design

Bind q in web/src/app/keyboard.ts (the #624 step 17 keyboard composition) to the existing quote-reply act for the current selection. Guards: no selection does nothing; never while typing (input/composer/contenteditable), during IME composition, or with modifiers. Listed in keyboard help.

## Acceptance Criteria

Real mouse selection + q opens the quoted reply on desktop and phone viewport; q in the composer types q; gate and land as usual.

## Notes

[watermelon-full-preview] mu-docview. Queued after read-next-finish and the moved-document fallback.

## Resolution

[watermelon-full-preview] Delivered on main by b7f8a28731a894cec3e9ece08ccf864410af2ba4 (quote act) and 8effc72ad6e4b657306d8557a5d9d670b7b60543 (q binding). The keymap chord runs quoteSelectionNow for the current selection, does nothing with no selection, and is listed in keyboard help. Tests: quoteSelectionNow opens the quick reply for held words (vibe-talk/tests/js/view/channel.test.mjs) and q quotes the selected words and types a q in a box (vibe-talk/tests/js/page/replies.mjs). Closed during GitHub triage (Request 098) alongside GitHub #311.
