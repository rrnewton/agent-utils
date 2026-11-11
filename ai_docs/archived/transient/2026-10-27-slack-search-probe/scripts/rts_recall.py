#!/usr/bin/env python3
"""Read-only: recall of Slack's Real-Time Search (assistant.search.context) against a ground truth.

GROUND is the JSONL ground_truth.py wrote: every message of one conversation in [OLDEST, LATEST).
"Ours" is the shared search language's rule for one word: its folded form is a substring of the
folded haystack (author + text), folding as src/search_query.rs does (NFD, marks dropped, NFC,
lower-case, the fold table, white space collapsed). Terms are chosen deterministically from the
ground truth (alphabetic words of 4+ letters in 5 to 40 messages, evenly spread over that range by
message count), unless TERMS names them.

For each term the search is paged to the end (at most 20 pages), narrowed to the conversation and
the window, sorted by time. Every call is paced and a rate limit waits and retries.

Usage: SLACK_SEARCH="<cli> <slack-message-object> search" rts_recall.py CHANNEL OLDEST LATEST GROUND OUT.json
"""
import json, os, re, shlex, subprocess, sys, time, unicodedata

channel, oldest, latest, ground_path, out_path = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
prefix = shlex.split(os.environ["SLACK_SEARCH"])
PACE = float(os.environ.get("PACE_S", 3.0))
DIACRITICS = ((0x300, 0x36F), (0x1AB0, 0x1AFF), (0x1DC0, 0x1DFF), (0x20D0, 0x20FF), (0xFE20, 0xFE2F))
FOLD = {"ß": "ss", "ς": "σ", "ſ": "s", "ϐ": "β", "ϑ": "θ", "ϕ": "φ", "ϖ": "π", "ϰ": "κ", "ϱ": "ρ", "ϵ": "ε",
        "ﬀ": "ff", "ﬁ": "fi", "ﬂ": "fl", "ﬃ": "ffi", "ﬄ": "ffl", "ﬅ": "st", "ﬆ": "st", "ı": "i", "ø": "o",
        "ł": "l", "đ": "d", "ħ": "h", "ŧ": "t", "µ": "μ", "ŉ": "ʼn", "և": "եւ", "ẚ": "aʾ", "ﬓ": "մն",
        "ﬔ": "մե", "ﬕ": "մի", "ﬖ": "վն", "ﬗ": "մխ", "ᲀ": "в", "ᲁ": "д", "ᲂ": "о", "ᲃ": "с", "ᲄ": "т",
        "ᲅ": "т", "ᲆ": "ъ", "ᲇ": "ѣ", "ᲈ": "ꙋ"}


def fold(text: str) -> str:
    """The shared language's fold (vibe-talk/src/search_query.rs): diacritics dropped, lower-cased,
    the table applied, white space collapsed. The probe's terms were all plain ASCII words, for
    which this and the fold it first ran with (every combining mark dropped) agree."""
    unmarked = unicodedata.normalize("NFC", "".join(
        c for c in unicodedata.normalize("NFD", text) if not any(a <= ord(c) <= b for a, b in DIACRITICS)))
    return re.sub(r"\s+", " ", "".join(FOLD.get(c, c) for c in unmarked.lower()))

ground = [json.loads(l) for l in open(ground_path)]
hay = {g["ts"]: fold(g["author"] + "\n" + g["text"]) for g in ground}
if os.environ.get("TERMS"):
    terms = os.environ["TERMS"].split(",")
else:
    df: dict[str, int] = {}
    for text in hay.values():
        for w in set(re.findall(r"[a-z]{4,}", text)):
            df[w] = df.get(w, 0) + 1
    pool = sorted((n, w) for w, n in df.items() if 5 <= n <= 40)
    step = max(1, len(pool) // 10)
    terms = [w for _, w in pool[::step]][:10]

calls: list[float] = []
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


def run(args: list[str]) -> dict[str, object]:
    for attempt in range(7):
        time.sleep(PACE)
        t0 = time.time()
        out = subprocess.run(args, capture_output=True, text=True)
        calls.append(time.time() - t0)
        if out.returncode == 0:
            answer: dict[str, object] = json.loads(out.stdout)
            return answer
        if "rate_limit" in out.stderr + out.stdout:
            limited.append(time.time())
            time.sleep(30 * (attempt + 1))
            continue
        sys.exit(f"cli failed rc={out.returncode}: {out.stderr[-400:]}")
    sys.exit("gave up after repeated rate limits")

def search(query: str, max_pages: int = 20) -> tuple[set[str], int, dict[str, object]]:
    token: str | None = None
    hits: list[dict[str, str]] = []
    pages = 0
    meta: dict[str, object] = {}
    while pages < max_pages:
        args = prefix + [f"--query={query}", f"--channel-id={channel}", f"--created-after={oldest}",
                         f"--created-before={latest}", "--sort=timestamp", "--sort-dir=asc",
                         "--output=json", "--columns=message_ts,thread_ts"]
        if token:
            args.append(f"--page-token={token}")
        d = run(args)
        pages += 1
        rows = rows_of(d)
        hits.extend(rows)
        meta = {k: v for k, v in d.items() if k != "data"}
        token = next_page(d)
        print(f"{query!r} page {pages} +{len(rows)} limited={len(limited)}", file=sys.stderr, flush=True)
        if not token or not rows:
            break
    return {h["message_ts"] for h in hits}, pages, meta

term_reports: list[dict[str, object]] = []
tot_ours = tot_both = 0
report: dict[str, object] = {"window": [oldest, latest], "ground_messages": len(ground), "terms": term_reports}
for term in terms:
    ours = {ts for ts, text in hay.items() if fold(term) in text}
    theirs, pages, meta = search(term)
    found = ours & theirs
    tot_ours += len(ours)
    tot_both += len(found)
    missed = sorted(ours - theirs, key=float)
    term_reports.append({
        "term": term, "ours": len(ours), "upstream": len(theirs), "both": len(found),
        "recall": round(len(found) / len(ours), 3) if ours else None,
        "upstream_not_ours": len(theirs - ours), "upstream_outside_ground": len(theirs - set(hay)),
        "pages": pages, "completeness": meta.get("completeness"),
        # Why each miss is a miss, by shape only (no text leaves this script): where the term sits.
        "misses": [{"infix": not re.search(r"(^|[^\w])" + re.escape(fold(term)), hay[ts]),
                    "chars": len(hay[ts]), "reply": bool(g.get("thread_ts") and g["thread_ts"] != g["ts"])}
                   for ts in missed for g in ground if g["ts"] == ts],
    })
calls.sort()
report["calls"] = len(calls)
report["rate_limited"] = len(limited)
report["seconds"] = round(sum(calls), 1)
report["p50_s"] = round(calls[len(calls) // 2], 2) if calls else None
json.dump(report, open(out_path, "w"), indent=1)
print(json.dumps({"terms": len(terms), "ours": tot_ours, "both": tot_both,
                  "recall": round(tot_both / tot_ours, 3) if tot_ours else None, "calls": len(calls),
                  "rate_limited": len(limited)}))
