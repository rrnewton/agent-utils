---
title: 'thread-list-first-read-reorder: on a phone the thread list''s first open reorders its cards when its read lands'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-23T23:26:14.894818342+00:00
updated_at: 2026-10-23T23:26:14.894818342+00:00
---

# Description

[opus 5.5] Found under #472 gate-flake-hardening. On a phone the thread list (Cmd+3, or the picker) is first drawn from the store: the cards in the store's order and without their summaries (132 px tall in the scroll-stability fixture). That first open also sends the first read of the threads (/timeline?view=threads&reason=side); when it lands the cards take the server's order and draw their summaries (175 px), so a reader at the foot sees the list change under them: in the fixture the first card on screen goes from t28 to t49. On a calm loopback host it lands within milliseconds; on a slow network the reader would see it. tests/scroll_stability_browser.py now holds the list from when that read has landed (it had failed on loaded gate hosts), so nothing pins this behaviour. Options for the owner: read the thread list in the background on a phone too, before it is opened; or draw the list only once its read has landed; or keep the reader's card in place across the reorder.
