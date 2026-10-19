---
title: 'speech-ticket-gone: a prepared read-aloud URL whose ticket the server no longer holds fails the read instead of preparing again'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T10:06:38.259250388+00:00
updated_at: 2026-10-19T10:06:38.259250388+00:00
---

# Description

[opus 5.5] Left over from #290 speech-ticket-refresh, which stops the page playing a prepared URL past its ticket's lifetime. A ticket can also go before that: the server restarts (every ticket an open page holds is gone), or it holds more than MAX_TICKETS (512) live tickets and evicts the oldest. The page still holds that URL as good until its deadline; a tap hands it to the player, the ticket route answers 404, and the read fails with 'that message could not be played.' and does not fall back. Fix in vibe-talk/web/voice.js: when a streamed part's response is a 404 before any audio arrived, drop that entry, prepare the row again once and play the new URL, or read it by the authenticated fetch when that fails; and a page that can tell the server restarted forgets what it prepared.

# Acceptance Criteria

A tap on a row whose prepared ticket the server no longer holds still reads it: prepared again once, else by the authenticated fetch. Page tests with the ticket route answering 404 (a restart, an eviction) and a refresh that fails; a Chromium check against the read-aloud fixture with a ticket dropped server-side.
