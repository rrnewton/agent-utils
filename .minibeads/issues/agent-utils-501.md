---
title: 'thread-bridge-gutter: on a phone the bridge hugs the left edge and a tap on it goes back to the thread''s card'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-25T11:45:32.694247698+00:00
updated_at: 2026-10-25T11:45:32.694247698+00:00
---

# Description

[muse watermelon] In the Threads view on a phone, an unpacked thread wastes a wide left gutter on its connector bridge: the pack triangle sits far from the left screen edge and the rows start further in still, narrowing every message. Push the bridge and the triangle as far left as they can go on narrow widths (the triangle a few pixels from the screen edge, its tap target at least 32px, the rows' indent still showing they belong to the thread), consistently wherever the bridge appears on a phone, and leave the desk layout as it is. Tapping the bridge line itself (not the triangle) scrolls smoothly back to that thread's card at the top of the thread, the card fully visible below the header, respecting reduced motion, with a generous invisible hit area along the line. The pack triangle, the pack stillness/slide and the card read dimming keep working as they did.

# Acceptance Criteria

At 393x852 and 412x915 the triangle's ink is a few px from the left edge on an unpacked thread, its target is at least 32px, the unpacked rows start materially further left than before and stay tied to their card by the bridge; the desk layout is unchanged. A tap on the bridge of a thread scrolled away from its card lands with the card fully visible at the head of the list, smoothly or at once under reduced motion; a tap on the triangle still packs the thread in place. Page tests (fake-DOM and Chromium) pin the geometry and the tap.
