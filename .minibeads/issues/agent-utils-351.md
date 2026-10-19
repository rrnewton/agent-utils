---
title: 'settings-polish: follow-ups from the Settings batch (#345-#349) review'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T01:49:17.542496075+00:00
updated_at: 2026-10-19T01:49:17.542496075+00:00
---

# Description

[opus 5.5] Review of the Settings batch (#345 settings-multi-column, #346 settings-collapsible, #347 settings-prose, #348 settings-dynamic-lists, #349 channel-settings-all) found these defects:

1. Put back in the Recently dismissed pop-out closes it with no focus to return to (the row that opened it is gone), so focus falls to the page body. Focus the next row, else the list, else its state line.
2. The dismissed-message pop-out dims the screen like a modal but is a plain dialog: Tab leaves it for the controls behind the dim and clicks go through the dim. Make it a real modal (aria-modal, focus kept inside, background inert or a click on the dim closes it), keeping Escape, Back and folding behaviour.
3. Each Recently dismissed row button is named by its meta line plus the whole message. Name it by the meta line and the first line only, keeping the full text in the row for find-in-page.
4. "All channels" is offered whenever there are two or more channels, even when none can take Clear unread or the token is read-only, which leaves nothing to do once Rename and Remove step aside. Offer it only when at least one channel can take Clear unread with this token.
5. "Expand all" and "Fold all" count the folds of hidden groups (the provider and message-cache groups when they are hidden). Count only visible folds.
6. Help for "recap refused" promises more than the page does: add that a refusal arriving more than 5 s after the recap leaves the line above the call reading "recap sent", as the README says.
7. The who-is-who list follows the channel of whichever column is active, so with two columns open it can switch to the right column's channel while its first line says it is about the channel you are reading; and its state line is no longer cleared when its channel changes. Fix both.

Each fix gets a page-suite test that fails before it (with negative controls where relevant), and browser checks where focus or modality is involved.
