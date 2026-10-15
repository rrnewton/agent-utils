---
title: 'late-counts-resurrect: a late read of All brings back a removed message or undoes an edit'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T05:48:32.159307484+00:00
updated_at: 2026-10-15T09:39:49.114619236+00:00
closed_at: 2026-10-15T09:39:49.114619056+00:00
---

# Description

Older than #308 live-reply-in-main (reproduced at ab8409bd and after #308; probes r6c.block.mjs and R4G in the #308 review scratch). The read of All that the page makes for Main's counts can be sent before an upstream removal or edit and land after Main's own read has applied it: the counts page then brings the removed message back onto Main, or puts back the text from before the edit, and both stay until the next full read. This is a different hazard from #310 stale-live-read-flicker (a late page hiding a message): here a late page resurrects a deleted message or undoes an edit. Fold the counts page only over store copies unchanged since it was sent (the same object check #308 added for the placing read, heldAsSent), and never re-add an id the stream removed after the read was sent; otherwise take only its counts. Also covers a bare reply edited upstream while a placing page is out (a bridge that streams its answer by editing it): after #308 it is placed in its thread but keeps the text it was first pushed with until the next read. Tests with negative controls for removal, edit and streaming edit.
