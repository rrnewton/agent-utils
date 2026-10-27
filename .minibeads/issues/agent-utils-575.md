---
title: 'mobile-keyboard-compose: the message box flush on a phone''s keyboard, and a way to write a new line there'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-27T12:43:18.901891796+00:00
updated_at: 2026-10-27T12:53:37.741559333+00:00
closed_at: 2026-10-27T12:53:37.741559223+00:00
---

# Description

[opus 5.5] The owner, 2026-10-27, on Android and iOS: tapping the message box scrolled it so that Send and some useless white space sat between the box and the keyboard, though the keyboard has its own send key; and there was no way to write a new line, since pressing Shift does not turn the keyboard's send key into a return.

Wanted:
1. With the on-screen keyboard up, the box's bottom flush with the keyboard's top, no wasted space, and the on-page Send hidden while the keyboard's key sends.
2. A way to write a new line from a phone's keyboard that works on both Gboard and iOS, without undoing #566 paste-never-sends.
3. Fake-DOM tests for the Enter policy and the keyboard-open layout; a Chromium touch walk at 393x852 and 412x915 with the keyboard emulated by shrinking the viewport, with screenshots.
