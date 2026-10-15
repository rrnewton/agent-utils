---
title: 'composer-grow-box: on desktop, a checkbox that lets the message box grow with its text'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T19:34:17.418589123+00:00
updated_at: 2026-10-15T19:34:17.418589123+00:00
---

# Description

[opus 5.5] Owner, 2026-10-15: on desktop, pasting a long message leaves the message box tiny, while the space left of Send is empty. Put a small checkbox (for example 'Grow') in that space; when checked, the box grows with typed or pasted text (CSS field-sizing: content where the browser supports it, otherwise resize to scrollHeight on input), up to about 60% of the viewport height, with inner scrolling beyond so Send stays visible. Remember the choice. Phone layout unchanged. Page tests and Chromium checks at 1280x800 and 1920x1080 with a long paste.
