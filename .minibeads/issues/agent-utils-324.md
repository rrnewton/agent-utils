---
title: 'live-pill-covers-row: the live pill covers the top row on a phone'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T06:03:00.417442984+00:00
updated_at: 2026-10-15T08:54:04.828397342+00:00
closed_at: 2026-10-15T08:54:04.828397202+00:00
---

# Description

Found by a read-only exercise of a Slack deployment on 2026-10-14 (the Slack move's parity stage). On a phone (412 px) the floating 'Live · updated' pill covers the top row: in a thread it hides the date separator and a clipped strip of text shows under the thread card; in Main it covers an unread chip. Keep the pill clear of the first row (reserve its space, or place it where it covers nothing interactive), with a Chromium check at 412 px in Main and in a thread.
