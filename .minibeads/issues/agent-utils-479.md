---
title: 'message-time-full-date: a message''s time says its full date on hover, focus and touch, and its day inline when not today'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-24T02:22:19.277386836+00:00
updated_at: 2026-10-24T02:51:02.330791825+00:00
closed_at: 2026-10-24T02:51:02.330791694+00:00
---

# Description

[muse watermelon] The owner: "I hate being disoriented when I jump to a search result about when it was from. I sometimes forget I can press the i key to expand and show message info/metadata. I would like a simple mouse hover on the time '11:09 PM' to show the full date."

Already there (`#260 timestamp-hover`): hovering a row's time shows its day in a native title ("Yesterday, Oct 11, Sun, 5:25 PM"), and a press-and-hold on the row opens the details sheet with the same words. Missing: the time is not keyboard-focusable, a tap or long-press on the time itself says nothing briefly, and a reader who never hovers still sees only an hour and minute for a message days old.

Wanted: hovering any message's time (channel rows, thread rows, cards where a time is shown, the transcript) shows the full local date and time, year only when not this year; the time is keyboard-focusable with an accessible name saying the same; on touch, tapping the time shows the full date briefly (the status line). Inline: a time not from today says its day inline (Yesterday / weekday within the week / Mon D, year when not this year), so a search landing is oriented without hovering. Page tests.

# Acceptance Criteria

Hover title, focus name and tap all say the full local date+time for the same message in the same words; inline day shown iff not today, freshened like the title so an overnight page does not lie; existing `#260` day words unchanged for the details sheet; page tests cover today/yesterday/week/older/other-year in a named zone.
