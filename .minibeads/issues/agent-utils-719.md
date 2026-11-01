---
title: 'vt-thread-unpack-loop: a thread longer than one page re-reads its newest page in a tight loop'
status: closed
priority: 0
issue_type: bug
created_at: 2026-11-01T08:52:51.594238188+00:00
updated_at: 2026-11-01T08:53:18.544527116+00:00
closed_at: 2026-11-01T08:53:18.544526976+00:00
---

# Description

[opus 5.5] The owner's deployed page, 2026-11-01 08:24Z: one client sent GET /api/v1/channels/<channel>/timeline?view=thread&reason=threads (cursor newest) about 3.5 times a second, every answer the same, from when the owner opened a thread until the page was reloaded by a rollback.

Cause: unpackNeedsRead treats a done unpack read as owed again whenever the thread's held replies are fewer than its count. A thread longer than one page never holds all its replies, so every redraw of its rows asks for its newest page again, and readUnpackThread's finally redraws them (the side cards, and a thread gathered under its root in Main by Reply, #410 inline-reply-editor). Reproduced in the page suite on 18.5 (d6402fa1) and 78b460b3 as well: it predates #624's view moves.

Fix: a done read is owed again only once the thread's count has grown past the count it had when that read landed (unpackReadCounts). Test: tests/js/page/replies.mjs, a gathered 40-reply thread with a two-reply newest page is read once while idle, and once more when a live reply grows it.
