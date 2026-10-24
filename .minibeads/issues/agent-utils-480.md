---
title: 'fold-one-message-key: Shift+O folds the selected message into a read-run line; open-upstream moves to Cmd+O'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-24T04:51:51.781163432+00:00
updated_at: 2026-10-24T06:53:06.914373624+00:00
closed_at: 2026-10-24T06:53:06.914373243+00:00
---

# Description

[muse watermelon] The owner: "Pressing 'o' to toggle shrink/expand is nice. But sometimes while I'm reading I want to pack the current thread denser without some global unpack change. I just want to work through one thread incrementally, then the next. I would like Shift+O to FULLY collapse a card into one of the '... N read messages ...' separators. That way I can selectively close up the parts of the thread I'm done with. The highlight should move to the message above, so I can just spam the key to go up and close up the thread until it hits the thread card at the top and does nothing. I see Shift+O is already bound to open the hyperlink; I'd like to change that to Cmd+O."

Wanted: Shift+O on a selected message folds that message into a separator line in Fold read's idiom (adjacent folded messages merge into one line with a count), then moves the selection to the message above. On the thread's root card it does nothing. Per-message folded state survives redraws; tapping or Enter on the line opens the run again, as fold-read lines do. Decide mark-read (probably yes if not already) and say which. Move "open upstream link" from Shift+O to Cmd+O (Ctrl+O off a Mac), outside text fields, preventing the browser's Open File; update Help, aria-keyshortcuts, README key table. Page tests.

# Acceptance Criteria

Shift+O folds one message, merges adjacent folds into one counted line, selection steps up, root card is a no-op, run reopens by tap/Enter, state survives a redraw/reload-of-rows; Cmd/Ctrl+O opens the upstream link and Shift+O no longer does; help/README/aria agree.
