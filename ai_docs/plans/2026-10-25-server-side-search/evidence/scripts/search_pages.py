#!/usr/bin/env python3
"""Read-only pilot: run one Slack search (Real-Time Search, assistant.search.context) and page it.

The pilot ran this through a vendor CLI whose `search` subcommand wraps assistant.search.context:
one call is one page, `--channel-id` adds `in:<#ID>`, and `--page-token` continues. Name that
command prefix in SLACK_SEARCH, for example SLACK_SEARCH="<cli> <slack-message-object> search".
Nothing here posts or changes anything upstream.

Usage: SLACK_SEARCH="..." [DUMP=file] search_pages.py MAX_PAGES QUERY [extra flags...]
Prints a JSON line: pages, hits, distinct timestamps, seconds, whether more pages remained, and the
completeness the service reported. With DUMP set, writes every hit's message_ts to that file, one a
line, for set comparison against a local copy (`compare_sets.py`).
"""
import json
import os
import shlex
import subprocess
import sys
import time

max_pages, query, extra = int(sys.argv[1]), sys.argv[2], sys.argv[3:]
prefix = shlex.split(os.environ["SLACK_SEARCH"])
token = None
hits = []
pages = 0
lat = []
meta = {}
err = ""
while pages < max_pages:
    args = prefix + [f"--query={query}", "--output=json", "--columns=message_ts,thread_ts"] + extra
    if token:
        args.append(f"--page-token={token}")
    t0 = time.time()
    out = subprocess.run(args, capture_output=True, text=True)
    lat.append(time.time() - t0)
    if out.returncode != 0:
        err = (out.stderr.strip().splitlines() or [f"rc {out.returncode}"])[-1][:200]
        break
    d = json.loads(out.stdout)
    pages += 1
    hits.extend(d.get("data", []))
    meta = {k: v for k, v in d.items() if k != "data"}
    pagination = d.get("pagination") or {}
    token = pagination.get("next_page_token")
    if not pagination.get("has_next_page") or not token or not d.get("data"):
        break
if os.environ.get("DUMP"):
    with open(os.environ["DUMP"], "w") as f:
        f.write("\n".join(h.get("message_ts", "") for h in hits))
print(json.dumps({
    "query": query, "extra": extra, "pages": pages, "hits": len(hits),
    "distinct": len({h.get("message_ts") for h in hits}),
    "seconds": round(sum(lat), 2), "per_call_s": round(sum(lat) / max(1, len(lat)), 2),
    "more": bool((meta.get("pagination") or {}).get("has_next_page")),
    "page_size": (meta.get("pagination") or {}).get("page_size"),
    "completeness": meta.get("completeness"), "error": err,
}))
