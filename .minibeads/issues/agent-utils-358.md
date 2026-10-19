---
title: 'held-done-relisted: under Hide read without threads, a to-do read landing while a Done is on its way lists its row again'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T09:03:20.951145898+00:00
updated_at: 2026-10-19T09:03:20.951145898+00:00
---

# Description

Found while finishing #327 done-races-remaining (2026-10-19); older than it. In a channel without threads under Hide read, a Done takes its row off the list at once. If a to-do read (the 45-second poll, or any other) is answered while that Done's write is still on its way, the server still lists the message, and the read draws its row back on the list, drawn Done (greyed). Once the Done is saved, the row stays listed, and counted in the head of the list ("3 of 3 still want attention"), until the next to-do read. Expected: a row whose messages all have a Done on its way stays off the list after such a read, and is not counted; a refusal then puts it back where it stands (#327 putBackRowsOff already places by message). A message whose Done is not on its way keeps today's behaviour. Page test with a negative control: Done held, poll answered, Done saved: the row is not listed and not counted; Done refused instead: the row is back where it stood, not Done.
