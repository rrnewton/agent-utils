---
title: 'reply-focus-flake: the reply-in-column check loses the box''s focus under load'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T04:34:14.286272817+00:00
updated_at: 2026-10-15T04:34:14.286272817+00:00
---

# Description

The browser check vibe-talk.check.reply-in-column-browser fails now and then under host load with 'narrowed: the box lost the focus on the way to the reply screen', whichever change is on top: it failed on the gates of #303 voice-post-prefix and #304 back-gesture-in-app (2026-10-14, load average about 110), neither of which touches reply focus, and passed when rerun alone (2 of 3 and 3 of 3). Find whether the page really drops the composer's focus when a narrow layout switches to the reply screen during a slow frame (a real bug the owner could hit on a phone), or whether the check reads focus before the transition settles; fix the page if it is real, otherwise make the check wait for the transition's end, not a fixed time. Prove it with the check run under CPU contention (for example 20 runs with the host loaded) passing every time.
