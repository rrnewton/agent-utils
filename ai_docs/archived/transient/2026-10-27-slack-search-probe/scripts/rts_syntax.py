#!/usr/bin/env python3
"""Read-only: which Slack search syntax works through Real-Time Search, one page per query.

Each query is narrowed to CHANNEL (--channel-id, unless the query names the channel itself) and the
window [OLDEST, LATEST); the hit count is set beside "ours" where the shared language can say it:
the same rule as rts_recall.py over the ground truth GROUND. Only counts leave this script.

Usage: SLACK_SEARCH=... rts_syntax.py CHANNEL CHANNEL_NAME OWNER_ID OLDEST LATEST GROUND
"""
import json, os, re, shlex, subprocess, sys, time, unicodedata
from typing import Callable
channel, name, owner, oldest, latest, ground_path = sys.argv[1:7]
prefix = shlex.split(os.environ["SLACK_SEARCH"])

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
hay = [(g, fold(g["author"] + "\n" + g["text"])) for g in ground]
def ours(pred: Callable[[dict[str, str], str], bool]) -> int:
    return sum(1 for g, h in hay if pred(g, h))

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

def search(query: str, narrow: bool = True, pages: int = 20) -> dict[str, object]:
    token: str | None = None
    n = 0
    for _ in range(pages):
        args = prefix + [f"--query={query}", f"--created-after={oldest}", f"--created-before={latest}",
                         "--output=json", "--columns=message_ts"] + ([f"--channel-id={channel}"] if narrow else [])
        if token:
            args.append(f"--page-token={token}")
        for attempt in range(6):
            time.sleep(3)
            r = subprocess.run(args, capture_output=True, text=True)
            if r.returncode == 0 or "rate_limit" not in r.stderr + r.stdout:
                break
            time.sleep(30 * (attempt + 1))
        if r.returncode != 0:
            return {"error": (r.stderr.strip().splitlines() or ["?"])[-1][:160]}
        d: dict[str, object] = json.loads(r.stdout)
        rows = rows_of(d)
        n += len(rows)
        token = next_page(d)
        if not token or not rows:
            break
    return {"hits": n}

def day(s: str) -> float:
    import calendar
    return calendar.timegm(time.strptime(s, "%Y-%m-%d"))
cases = [
    ("gate", True, ours(lambda g, h: "gate" in h)),
    ('"the gate"', True, ours(lambda g, h: "the gate" in h)),
    (f"in:#{name} gate", False, ours(lambda g, h: "gate" in h)),
    (f"gate from:<@{owner}>", True, None),
    ("gate after:2026-10-26", True, ours(lambda g, h: "gate" in h and float(g["ts"]) >= day("2026-10-27"))),
    ("gate before:2026-10-27", True, ours(lambda g, h: "gate" in h and float(g["ts"]) < day("2026-10-27"))),
    ("gate on:2026-10-26", True, ours(lambda g, h: "gate" in h and day("2026-10-26") <= float(g["ts"]) < day("2026-10-27"))),
    ("gate OR report", True, ours(lambda g, h: "gate" in h or "report" in h)),
    ("gate report", True, ours(lambda g, h: "gate" in h and "report" in h)),
    ("gate -report", True, ours(lambda g, h: "gate" in h and "report" not in h)),
    ("gat*", True, None),
    ("gat", True, None),
    ("andoff", True, ours(lambda g, h: "andoff" in h)),
    ("handoff", True, ours(lambda g, h: "handoff" in h)),
]
rows: list[dict[str, object]] = []
for query, narrow, expected in cases:
    got = search(query, narrow)
    rows.append({"query": query.replace(owner, "<owner>").replace(name, "<channel>"), "narrowed": narrow, "ours": expected, **got})
    print(json.dumps(rows[-1]), flush=True)
