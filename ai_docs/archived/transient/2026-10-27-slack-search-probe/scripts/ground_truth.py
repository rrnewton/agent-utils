#!/usr/bin/env python3
"""Read-only: every message of one Slack conversation in a time window, replies included.

Walks conversations.history over [OLDEST, LATEST) through the vendor CLI's history subcommand,
then conversations.replies for every thread root it saw (and every root whose thread could hold
replies in the window: roots up to LOOKBACK seconds older are read too), keeping replies created
in the window. Writes JSONL: ts, thread_ts, author, text (plain text, links as "label (url)").

Usage: SLACK_LIST="<cli> <slack-message-object> list" ground_truth.py CHANNEL OLDEST LATEST OUT.jsonl
"""
import json, os, shlex, subprocess, sys, time

channel, oldest, latest, out_path = sys.argv[1], float(sys.argv[2]), float(sys.argv[3]), sys.argv[4]
lookback = float(os.environ.get("LOOKBACK", 7 * 86400))
prefix = shlex.split(os.environ["SLACK_LIST"])
COLS = "message_ts,thread_ts,reply_count,author_name,author_user_id,subtype,content,attachments"
calls: list[float] = []

PACE = float(os.environ.get("PACE_S", 2.0))
limited: list[float] = []


def rows_of(answer: dict[str, object]) -> list[dict[str, str]]:
    """The answer's rows, every value as text ("" for none)."""
    data = answer.get("data")
    if not isinstance(data, list):
        return []
    return [{str(k): "" if v is None else str(v) for k, v in row.items()} for row in data if isinstance(row, dict)]


def next_page(answer: dict[str, object]) -> str | None:
    """The token of the answer's next page, when there is one."""
    pagination = answer.get("pagination")
    if not isinstance(pagination, dict) or not pagination.get("has_next_page"):
        return None
    token = pagination.get("next_page_token")
    return token if isinstance(token, str) and token else None


def run(args: list[str]) -> "subprocess.CompletedProcess[str]":
    """One CLI call, paced; a rate limit waits and retries (at most 6 times), and is counted."""
    for attempt in range(7):
        time.sleep(PACE)
        t0 = time.time()
        out = subprocess.run(args, capture_output=True, text=True)
        calls.append(time.time() - t0)
        if out.returncode == 0:
            return out
        if "rate_limit" in out.stderr or "rate_limit" in out.stdout:
            limited.append(time.time())
            time.sleep(30 * (attempt + 1))
            continue
        sys.exit(f"cli failed rc={out.returncode}: {out.stderr[-400:]}")
    sys.exit("gave up after repeated rate limits")

def page(extra: list[str]) -> list[dict[str, str]]:
    token: str | None = None
    rows: list[dict[str, str]] = []
    while True:
        args = prefix + [f"--channel-id={channel}", "--limit=200", "--output=json",
                         "--content-format=text", f"--columns={COLS}"] + extra
        if token:
            args.append(f"--page-token={token}")
        out = run(args)
        d: dict[str, object] = json.loads(out.stdout)
        got = rows_of(d)
        print(f"call {len(calls)} {' '.join(extra)} rows+={len(got)} limited={len(limited)}", file=sys.stderr, flush=True)
        if d.get("dropped_row_count"):
            print("dropped rows:", d["dropped_row_count"], file=sys.stderr)
        rows.extend(got)
        token = next_page(d)
        if not token:
            return rows

top = page([f"--oldest={oldest - lookback:.6f}", f"--latest={latest:.6f}"])
msgs: dict[str, dict[str, str]] = {}
roots: list[str] = []
for r in top:
    created = float(r["message_ts"])
    if oldest <= created < latest:
        msgs[r["message_ts"]] = r
    if r.get("reply_count", "") not in ("", "0"):
        roots.append(r["message_ts"])
for root in roots:
    for r in page([f"--thread-ts={root}"]):
        created = float(r["message_ts"])
        if r["message_ts"] != root and oldest <= created < latest:
            msgs[r["message_ts"]] = r
with open(out_path, "w") as f:
    for ts in sorted(msgs, key=float):
        r = msgs[ts]
        f.write(json.dumps({"ts": ts, "thread_ts": r.get("thread_ts"), "author": r.get("author_name") or "",
                            "subtype": r.get("subtype"), "text": r.get("content") or "",
                            "attachments": r.get("attachments")}) + "\n")
calls.sort()
print(json.dumps({"messages": len(msgs), "top_level_read": len(top), "threads": len(roots),
                  "calls": len(calls), "rate_limited": len(limited), "seconds": round(sum(calls), 1),
                  "p50_s": round(calls[len(calls) // 2], 2)}))
