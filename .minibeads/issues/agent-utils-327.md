---
title: 'done-races-remaining: four smaller Done races left after #306 and #311'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T13:11:36.510172499+00:00
updated_at: 2026-10-15T13:11:36.510172499+00:00
---

# Description

Found in the final reviews of #306 done-stale-across-views / #311 sent-read-waits-for-done (2026-10-15); behaves the same on main before those changes. Remaining Done races: (1) a thread opened in the right column for the first time while a Done made in the left column is being saved draws the message as not Done even after the Done is saved; (2) Through here does not wait for a Done still being saved on a row it covers; (3) an Undo pressed while a send's own Done (#300 reply-marks-read) is being saved still asks the chat service; (4) under Hide read in a channel without threads, a refused Done keeps the row off the list until the next read. Each needs a page test with a negative control; (2) and (3) should reuse doneSaved(ids) from #295 shift-e-read-collapse.
