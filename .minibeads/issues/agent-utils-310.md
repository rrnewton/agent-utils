---
title: 'stale-live-read-flicker: a live message drawn in All vanishes for a few frames when an earlier live read lands after it'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-14T19:48:50.922088080+00:00
updated_at: 2026-10-14T21:43:42.541909131+00:00
---

# Description

Found in review of #299 all-thread-strip (fix round 2). In a channel with threads, a live message is drawn the moment the stream delivers it (receiveLiveMessage -> appendToColumns), and the page then reads the channel again (loadDiscord, reason=live). When a second live message arrives while the first message's read is still on the wire, that read's answer, taken before the second message existed, lands after the stream has drawn it, and loadTimeline -> foldLateTimelinePage -> catchUpHeldView -> renderCachedTimeline -> replaceChildren redraws All without it for about 13 frames, until the second message's own read puts it back. A message the reader saw arrive disappears and comes back, against #280 scroll-stability (nothing moves under a reader who did not move). The review reproduced it at 18f3046e (main before #298/#299) and on the #298/#299 branch, 4 of 4 runs each, with live reads answered 150 ms late, and caught the removal on the path above with a DOM-mutation stack trace. Expected: a late answer never takes away a message the stream delivered after that read was asked (merge rather than replace: keep held messages newer than the read's snapshot). tests/all_thread_strip_browser.py now waits for each push's read to be answered before the next push and asserts only on a state that holds for 30 frames, so it no longer trips on this; a check for the fix should push two messages about 13 ms apart with the first message's read answered late, and require the second message's row in every frame.
