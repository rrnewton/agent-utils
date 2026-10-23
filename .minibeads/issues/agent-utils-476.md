---
title: 'scroll-older-replies-flake: scroll-stability''s older-replies hold failed once under load'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
- validation
created_at: 2026-10-23T23:26:16.373474047+00:00
updated_at: 2026-10-23T23:26:16.373474047+00:00
---

# Description

[opus 5.5] Found under #472 gate-flake-hardening; not seen in a gate. Under a process-group freezer (50 ms of every 100 stopped, 400 ms every 15th period), one of about 25 runs of tests/scroll_stability_browser.py on the pre-#472 page failed: 'phone-412: older replies coming in above moved the reader: row 220 at 141.7px became row 218 at -126.1px'. The reader's row was measured right after scrolling the long thread to its head, at 141.7 px below the list's top, so something other than a row filled the space above it (a loading line, or the card at the top); once the older page landed, rows 218 and 219 filled it. The check takes 'the first row on screen' as the reader's row, so a row coming into blank space above reads as a move. Either measure the reader's own row by id, or wait for whatever the head of the thread draws before measuring.
