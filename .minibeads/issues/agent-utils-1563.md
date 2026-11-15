---
title: 'thread-picker-names-and-width: slug plus readable title in the picker and header, names from the cache'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-15T12:00:00.000000000+00:00
updated_at: 2026-11-15T12:00:00.000000000+00:00
---

# Description

Owner UI request 087 (fast-tracked), widened by owner 088. The thread picker from `#1550 thread-list-pins` (landed 3a338f8d + 35789200) is too narrow on desktop, and it shows tiny prefixes of the description instead of thread names/slugs; names are missing inside threads too.

- On desktop, widen the picker pop-up enough for a slug plus a readable title, clamped to the viewport. On a phone (390 px) it stays full width.
- Each thread row, and the header inside an open thread, shows the thread's slug (`display_name`, or `@agent` for a bang-bound thread) plus a readable title (the server's title, up to 80 characters). A thread with no name yet shows the title alone, never a 28-character prefix.
- Naming itself is starved: the live instance named 40 threads in two days against 79 channel pauses of 600 s, because each naming read competes for the Slack `conversations.replies` bucket and loses. The model answers every call. Fix naming to read the local cache first (below), backfill unnamed threads through the existing naming path, and raise a visible health warning when naming starves, not only a log line.

Earlier work: `#182 thread-display-names` (the slug and sentence, their schedule), `#194 thread-picker-polish` (one name everywhere), `#1550 thread-list-pins` (the pop-up this widens).

# Design

Two commits, each gated and landed on its own, picker first.

1. Picker and header. The pop-up is `min(560px, calc(100vw - 16px))`. One naming rule in the thread list (`threadSlug` / `threadTitle` / the full form of `threadName`): slug first, then title, joined for the pop-up rows (facts, bold slug, title, wrapping instead of one ellipsised line), the select's options, the thread cards, and — through the card view's slug — the header at the top of an open thread, whose root body is led by the slug. No name filed yet: the full title alone. The finite form of the name (the picker's closed face, prose mentions) keeps its short slug-first reading.
2. Cache-first naming. The old policy — a thread is named only from the upstream read made at its turn — changes to: named from the cache's proven-whole copy at its turn, or that read. `name_turn` first asks the message cache for the thread whole and fresh (the same proof a page read uses); a proven copy is named with no upstream call, consuming neither the per-channel naming interval nor the 600 s pause. Anything else takes today's upstream path unchanged (background priority, interval, pause, budgets). Server-scheduled and cache-first throughout: no page-triggered upstream reads (`#1227`). Health: thread-naming counters (unnamed due, unnamed overdue past an hour, paused channels, consecutive pauses) join the one `summarizer_health` diagnostics object shared with the summariser fallback work, loud past an hour unnamed or three consecutive pauses, and the page shows the same warning.

# Acceptance Criteria

- Browser test at 390 px and desktop: pop-up width clamped as above, slug plus full title on a named row, full title alone on an unnamed row, header leading with the slug; screenshots at both sizes, looked at.
- Naming tests: a proven-whole cache copy names with zero upstream calls and no interval or pause spent; a cache miss takes the upstream path; a channel paused for upstream still names from the cache.
- Health: the diagnostics surface reports the naming counters and goes loud on the stated thresholds.

# Notes

No live configuration change: no interval value fixes a starved shared bucket. Sibling lanes: the Settings view and the Discord row grouping touch other files; the shared health surface is agreed with the summariser-fallback lane, which folds these counters into its one headline rather than adding a second warning.

## Resolution

[watermelon-full-preview] Landed in two gated commits on github/main: the picker and header as 18caf792 (gate a70183a5), cache-first naming as 8db6490b (gate 33b870e2). Screenshots looked at: /tmp/shots-pickername/threadpin-desk-1-popup-bang-pinned.png, threadpin-phone-1-popup-bang-pinned.png, threadpin-desk-4-thread-header-slug.png. Browser tests at 390x852 and 1280x800 plus the naming suites pass; the full validate DAG is green twice. The thread_naming counters join the one summarizer_health diagnostics surface; the page banner is the fallback lane's fold-in of the same counters.
