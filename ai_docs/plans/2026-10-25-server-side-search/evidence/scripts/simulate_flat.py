#!/usr/bin/env python3
"""Replay Slack Flat ("All") paging over a channel's real shape, to see what All can ever load.

Input: a TSV of the channel's top-level messages, NEWEST FIRST, one a line:
    message_id <TAB> created_ms <TAB> reply_count <TAB> thread_last_activity_ms
(the pilot exported it from a private copy of the server's message cache; reply_count 0 for a
message that is not a thread root).

The model is `HttpSlackClient::flat` in vibe-talk/src/slack/timeline.rs: each page is the next
`limit` top-level messages, plus the replies of at most FLAT_MAX_THREADS of the threads rooted on
it, the most recently active first, at most FLAT_MAX_REPLIES each. Replies of the other threads on
that page are never read by any page.

Usage: simulate_flat.py ROWS.tsv [LIMIT]
"""
import json
import sys

FLAT_MAX_THREADS = 10
FLAT_MAX_REPLIES = 50

path = sys.argv[1]
limit = int(sys.argv[2]) if len(sys.argv) > 2 else 50
rows = []
for line in open(path):
    if line.strip():
        mid, created, count, activity = line.rstrip("\n").split("\t")
        rows.append((mid, int(created), int(count), int(activity)))
pages = []
loaded = 0
for at in range(0, len(rows), limit):
    page = rows[at:at + limit]
    roots = sorted((r for r in page if r[2] > 0), key=lambda r: -r[3])
    read = sum(min(r[2], FLAT_MAX_REPLIES) for r in roots[:FLAT_MAX_THREADS])
    dropped = sum(r[2] for r in roots[FLAT_MAX_THREADS:])
    pages.append({"top_level": len(page), "threads": len(roots), "replies_loaded": read,
                  "replies_never_loaded": dropped, "notice": len(roots) > FLAT_MAX_THREADS})
    loaded += len(page) + read
print(json.dumps({
    "top_level": len(rows), "replies": sum(r[2] for r in rows),
    "channel_total": len(rows) + sum(r[2] for r in rows),
    "all_view_loads_at_most": loaded, "pages": pages,
}, indent=1))
