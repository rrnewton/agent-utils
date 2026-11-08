---
title: 'quote-reply-key: q for quoted reply on a selection'
status: open
priority: 1
issue_type: task
external_ref: gh-311
created_at: 2026-11-08T00:19:17.835224052+00:00
updated_at: 2026-11-08T00:19:17.835224052+00:00
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
