---
title: 'chip-states: the Threads view''s heading chips show on and off alike, and are renamed Main channel and Unpack msgs'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-22T13:53:49.074082653+00:00
updated_at: 2026-10-22T16:06:12.191688034+00:00
closed_at: 2026-10-22T16:06:12.191687924+00:00
---

# Description

[opus 5.5] The owner, 2026-10-22: "The Main messages toggle in Thread list view (Cmd 3) is working. But it does not change its visual style when it is toggled on off. I would like to change the names on those buttons too. Main channel instead of main messages. And Unpack msgs." Cause: #419 threads-view-main-messages added #side-main with aria-pressed but no pressed style; Unread only (#259) and Unpack all (#404) each carry their own copy of the pressed rule.

# Design

One pressed rule for every chip with aria-pressed (.chip[aria-pressed="true"]), the style Unread only and Unpack all already wear (accent ring, accent tint, ink words), replacing their per-id copies, so a chip added later is pressed the same way. Rename the labels; the phone keeps a short form through .chip-more.

# Acceptance Criteria

Main channel, Unpack msgs and Unread only all look pressed when aria-pressed is true and at rest when false, by one shared rule; labels, titles, key descriptions, ? help, README and tests say Main channel and Unpack msgs. Page tests with negative controls; Chromium at 1280x800 and 393x852 measuring each chip on and off.
