---
title: 'vt-older-message-dates: a message time older than today says its weekday or its date'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-01T21:11:25.457647536+00:00
updated_at: 2026-11-01T21:17:01.111805883+00:00
closed_at: 2026-11-01T21:17:01.111805733+00:00
---

# Description

[claude-opus-5-5] Owner GO 2026-11-01 12:04 ET, verbatim: 'I want "03:23 PM" to say "Fri 03:23 PM" if it's 6 days or less in the past (i.e. should not say today's day-of-week) and "10/30 03:23 PM" if it's older.' Today = time only; 1-6 calendar days ago = weekday + time; 7+ days = M/D + time, in the viewer's own calendar. Every message time label; the hover keeps its full timestamp.
