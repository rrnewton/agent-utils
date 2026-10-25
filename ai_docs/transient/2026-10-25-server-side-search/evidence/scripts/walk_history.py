#!/usr/bin/env python3
"""Read-only pilot: page one Slack conversation's history (conversations.history) back to its start.

The pilot ran this through a vendor CLI whose `list` subcommand wraps conversations.history and
answers at most 20 rows a call with an opaque next-page token. Name that command prefix in
SLACK_LIST, for example SLACK_LIST="<cli> <slack-message-object> list". Nothing here posts or
changes anything upstream.

Usage: SLACK_LIST="..." walk_history.py CHANNEL_ID MAX_PAGES
Prints one line per page and a JSON summary: pages, messages, thread roots, the sum of the roots'
reply_count, the oldest timestamp, whether the start was reached, and per-call latency.
"""
import json
import os
import shlex
import subprocess
import sys
import time

channel, max_pages = sys.argv[1], int(sys.argv[2])
prefix = shlex.split(os.environ["SLACK_LIST"])
token = None
total = roots = replies = pages = 0
oldest = None
more = True
lat = []
started = time.time()
while pages < max_pages:
    args = prefix + [f"--channel-id={channel}", "--limit=20", "--output=json",
                     "--columns=message_ts,reply_count,subtype"]
    if token:
        args.append(f"--page-token={token}")
    t0 = time.time()
    out = subprocess.run(args, capture_output=True, text=True)
    lat.append(time.time() - t0)
    if out.returncode != 0:
        print("error", out.returncode, out.stderr[:300])
        break
    d = json.loads(out.stdout)
    rows = d.get("data", [])
    pages += 1
    total += len(rows)
    for r in rows:
        ts = r.get("message_ts")
        oldest = ts if oldest is None or float(ts) < float(oldest) else oldest
        if r.get("reply_count"):
            roots += 1
            replies += int(r["reply_count"])
    pag = d.get("pagination", {})
    token = pag.get("next_page_token") or None
    more = bool(d.get("has_more") or pag.get("has_next_page") or token)
    print(f"page {pages} rows {len(rows)} oldest {oldest} {lat[-1]:.2f}s more={more}", flush=True)
    if not more:
        break
lat.sort()
print(json.dumps({
    "pages": pages, "messages": total, "thread_roots": roots, "replies_by_reply_count": replies,
    "oldest_utc": time.strftime("%Y-%m-%d %H:%M", time.gmtime(float(oldest))) if oldest else None,
    "reached_start": not more, "seconds": round(time.time() - started, 1),
    "p50_s": round(lat[len(lat) // 2], 2) if lat else None, "max_s": round(lat[-1], 2) if lat else None,
}))
