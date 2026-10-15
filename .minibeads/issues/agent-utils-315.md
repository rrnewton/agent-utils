---
title: 'removed-reply-target: a reply to a message removed upstream goes to the main channel'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T05:48:32.156222900+00:00
updated_at: 2026-10-15T05:48:32.156222900+00:00
---

# Description

Found by the #308 live-reply-in-main review (probe r6r.block.mjs in the reviewer's scratch, not in the repo). Since #308, a stream removal of a message that arrived without its thread drops it from the page's unplaced set at once, but the bare copy stays in the store until the read behind the removal lands. Meanwhile replyRoute and placeReplyTarget treat it as placed with no thread, so a Reply to it is posted to the main channel with reply_to naming the removed message, with no read first; during an outage that window has no end. Before #308 the message was still unplaced, so Send read All first, did not find it, and refused ('Not sent. The message you answered came in live...'). Fix: keep a set of removed ids that placeFromPage skips and that replyRoute and placeReplyTarget treat as not placeable (refuse the reply, saying the message was removed), rather than reusing 'not unplaced', which other code reads as 'placed'. Test with the reply sent while the read behind the removal is held and while reads fail.
