---
title: 'read-strip-followups: Stop for a one-off read, Play told from Stop, a strip held on screen, the grip''s keys in Help'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- read-aloud
created_at: 2026-10-23T13:06:41.436672368+00:00
updated_at: 2026-10-23T14:41:34.769179984+00:00
closed_at: 2026-10-23T14:41:34.769179914+00:00
---

# Description

[muse watermelon] Follow-ups from #442 read-strip-order's handoff:

- a one-block read started with the s key outside reading mode has no bar Stop, and Read then enters reading mode instead of stopping; make Stop available for that case too;
- while a read is held, the strip's Play and Stop share the warn colour and differ only by word vs square; make them distinguishable at a glance;
- a dragged desktop strip can drift partly off-screen when the chips row or composer grows without a window resize; re-clamp on those layout changes;
- Help's keyboard list lacks the strip grip's keys (arrows, Shift+arrows, Home).

# Acceptance Criteria

Page tests: a read started with s outside reading mode can be stopped from the bar and does not enter reading mode; a held read's Play and Stop differ in more than word vs square; a moved strip is pulled back inside the window when the chips row or composer grows it off, with no window resize; Help's list of keys names the grip's arrows, Shift+arrows and Home. Chromium screenshots at 393x852 and desktop, both schemes.
