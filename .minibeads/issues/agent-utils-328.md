---
title: 'removal-outside-window: a reported removal of a message older than the newest page''s window does not stick'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T13:11:36.513662710+00:00
updated_at: 2026-10-15T13:11:36.513662710+00:00
---

# Description

Found in the final review of #316 late-counts-resurrect / #310 stale-live-read-flicker (2026-10-15); older than both. (1) vibe-talk/README.md (the #316 paragraph, around 'the first newest page to land takes the message out, even one sent before the removal') and the #316 close note say a newest page that lands after a reported removal takes the message out whatever it carries; that is not true for a message older than that page's window (has_more): the page cannot speak for it, so it stays. Either make the removal stick for messages outside the page's window (the stream reported it removed) or narrow both sentences. (2) A message the stream left unplaced that the store no longer holds is put back by the reply-place read as a new reply, adding one to its root's count, so it may be counted twice. Page tests with negative controls.
