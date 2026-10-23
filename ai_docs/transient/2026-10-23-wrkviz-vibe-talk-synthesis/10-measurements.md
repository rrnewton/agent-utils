# Appendix 10: Measurements, with the scripts that made them

Part of `#449 wrkviz-vibe-talk-synthesis`. Every number this proposal calls "measured" came from one
of the runs below, on 2026-10-23, on the host where the coding agents run. All were read-only:
nothing was written outside `/tmp`, no service was contacted, and the private archive was read from
its compressed bootstrap and its usage receipts. Paths to private data are shown as variables.

## 1. Transcript archive volume

The owner's private combined archive (12 teams, 60 days). Its schema-3 bootstrap was decompressed to
`/tmp` and its `stats` and per-stream record counts read:

```
events 499,155   user_prompts 9,001   agent_responses 119,657   tool_calls 301,532
inter_agent_messages 68,948   external_messages 17   active_agents 2,912
records by kind: event 499,155  search_record 415,714  edge 69,955  phase 18,371  activity_bin 2,852
range 2026-07-20T19:09Z .. 2026-09-18T15:35Z
```

Summary artifacts present: 1,849 phase projections, 420 agent projections, rollups 56 hourly,
26 daily, 4 weekly, 4 monthly, 4 quarterly (counted with `find` under `teams/*/summary_data`).

## 2. Summarizer tokens and latency, from usage receipts

```python
# receipts.py ARCHIVE: totals per cache kind
import json,glob,sys,statistics as st
from datetime import datetime
A=sys.argv[1]
for kind in ("cache","name_cache"):
    rows=[]
    for f in glob.glob(f"{A}/teams/*/summary_data/{kind}/_usage/receipts/*.json"):
        d=json.load(open(f))
        if d.get("status")!="completed": continue
        u=d.get("usage") or {}
        t0=datetime.fromisoformat(d["started_at"]);t1=datetime.fromisoformat(d["completed_at"])
        rows.append((len(d.get("input_hashes",[])),u.get("input_tokens",0),u.get("output_tokens",0),(t1-t0).total_seconds(),d.get("backend"),d.get("model")))
    if not rows: continue
    jobs=sum(r[0] for r in rows); inp=sum(r[1] for r in rows); out=sum(r[2] for r in rows)
    lat=[r[3] for r in rows]
    print(kind,"batches",len(rows),"jobs",jobs,"input",inp,"output",out,"in/job",inp//max(jobs,1),"out/job",out//max(jobs,1),
      "batch latency median",round(st.median(lat),1),"p90",round(sorted(lat)[int(.9*len(lat))],1),"jobs/batch median",st.median([r[0] for r in rows]))
    from collections import Counter
    print("  models",Counter((r[4],r[5]) for r in rows).most_common(5))
```

Output:

```
cache       batches 1194  jobs 2425  input 43,324,061  output 3,848,435  in/job 17,865  out/job 1,586
            batch latency median 23.4 s  p90 96.4 s; models: codex/gpt-5.5 843, claude/claude-sonnet-5 351
name_cache  batches 242   jobs 640   input 8,855,902   output 271,700    in/job 13,837  out/job 424
            batch latency median 18.0 s  p90 30.3 s
```

Single-job batches only, split by backend (`receipts2.py`, the same loop filtered on
`len(input_hashes) == 1` and `backend`):

```
cache       codex   single-job 618  latency p50 20.2 s  p90 27.9 s  in/job p50 16,919  out/job p50 1,168
cache       claude  single-job 99   latency p50 24.2 s  p90 58.3 s  in/job p50 10,521  out/job p50 1,731
name_cache  codex   single-job 20   latency p50 10.3 s  p90 14.9 s
name_cache  claude  single-job 104  latency p50 20.7 s  p90 35.0 s
```

These latencies include starting a coding CLI per batch; a direct API call would be faster.

## 3. Raw transcript bytes touched in 24 hours, and whole-file parse time

```python
import os,time,json,glob,sys
roots=[os.path.expanduser("~/.claude/projects"), os.path.expanduser("~/.codex/sessions")]
cut=time.time()-86400
for root in roots:
  files=[];total=0
  for dp,dn,fn in os.walk(root):
    for f in fn:
      if f.endswith(".jsonl"):
        p=os.path.join(dp,f)
        try: st=os.stat(p)
        except OSError: continue
        if st.st_mtime>=cut: files.append(p); total+=st.st_size
  t0=time.perf_counter(); lines=0; recs=0; sub=0
  for p in files:
    if "subagents" in p: sub+=1
    with open(p,'rb') as fh:
      for line in fh:
        lines+=1
        try: json.loads(line); recs+=1
        except Exception: pass
  print(root.replace(os.path.expanduser("~"),"~"),"files touched 24h",len(files),"of which subagent files",sub,"MB",round(total/1e6,1),"lines",lines,"parse s",round(time.perf_counter()-t0,2))
```

Output:

```
~/.claude/projects  files touched 24h 39 (16 subagent files)  2456.4 MB  1,732,393 lines  parse 16.62 s
~/.codex/sessions   files touched 24h 126                     5689.3 MB  1,313,439 lines  parse 24.88 s
```

"Touched" means modified in the last 24 hours; the files are whole sessions, so most of those bytes
are older history. That is the point: re-parsing whole files every poll costs tens of seconds, and a
byte cursor makes a poll cost only what was appended.

## 4. Deterministic voice-context preparations on a cache-shaped SQLite store

Synthetic data: 30 days, five channels, 90 messages per channel per day (450 a day, the measured
heavy day), bodies of 30 to 400 random words; the cache's time and thread indexes and an FTS5
trigram index. Python `sqlite3`, run isolated (`python3 -I`).

```python
import sqlite3,random,time,os,string
random.seed(7)
p="/tmp/cl-synth/bench.sqlite3"
if os.path.exists(p): os.remove(p)
db=sqlite3.connect(p)
db.executescript("""
CREATE TABLE message(row_id INTEGER PRIMARY KEY, chan INTEGER, message_id TEXT, created_ms INTEGER, thread_id TEXT, author TEXT, content TEXT, search_text TEXT);
CREATE INDEX message_by_time ON message(chan, created_ms);
CREATE INDEX message_by_thread ON message(chan, thread_id, created_ms);
CREATE VIRTUAL TABLE message_fts USING fts5(search_text, content='message', content_rowid='row_id', tokenize='trigram', detail=full);
CREATE TRIGGER ai AFTER INSERT ON message BEGIN INSERT INTO message_fts(rowid,search_text) VALUES(new.row_id,new.search_text); END;
CREATE TABLE summary(level TEXT, chan INTEGER, start_ms INTEGER, end_ms INTEGER, watermark INTEGER, text TEXT, PRIMARY KEY(level,chan,start_ms));
""")
words=[ ''.join(random.choice(string.ascii_lowercase) for _ in range(random.randint(3,9))) for _ in range(3000)]
now=1_800_000_000_000; DAY=86_400_000
rows=[]; n=0
for day in range(30):
  for chan in range(5):
    for i in range(90):  # 450/day across 5 channels (heavy day)
      t=now-day*DAY-random.randint(0,DAY)
      th=f"t{chan}-{day}-{random.randint(0,12)}"
      body=' '.join(random.choice(words) for _ in range(random.randint(30,400)))
      n+=1; rows.append((chan,f"m{n}",t,th,f"agent{random.randint(1,4)}",body,body.lower()))
t0=time.perf_counter(); db.executemany("INSERT INTO message(chan,message_id,created_ms,thread_id,author,content,search_text) VALUES(?,?,?,?,?,?,?)",rows); db.commit()
print("rows",n,"insert s",round(time.perf_counter()-t0,2),"db MB",round(os.path.getsize(p)/1e6,1))
def timeit(label,f,reps=20):
  f(); ts=[]
  for _ in range(reps):
    a=time.perf_counter(); r=f(); ts.append(time.perf_counter()-a)
  ts.sort(); print(f"{label}: p50 {ts[len(ts)//2]*1000:.1f} ms, p90 {ts[int(len(ts)*.9)]*1000:.1f} ms", r)
def last24():
  cur=db.execute("SELECT chan,thread_id,message_id,created_ms,author,substr(content,1,160) FROM message WHERE created_ms>=? ORDER BY chan,thread_id,created_ms",(now-DAY,))
  out=[]; nmsg=0
  for r in cur: nmsg+=1; out.append(f"[{r[2]} | {r[3]} | {r[4]}] {r[5]}")
  return nmsg, len('\n'.join(out))
def hourly_counts():
  return len(db.execute("SELECT chan,(created_ms/3600000) h,count(*),max(created_ms) FROM message WHERE created_ms>=? GROUP BY chan,h",(now-2*DAY,)).fetchall())
def fullday_text():
  return sum(len(r[0]) for r in db.execute("SELECT content FROM message WHERE created_ms>=?",(now-DAY,)))
def fts():
  w=random.choice(words)
  return len(db.execute("SELECT rowid FROM message_fts WHERE message_fts MATCH ? LIMIT 50",(f'"{w}"',)).fetchall())
timeit("24h index lines (id,time,author,160 chars)",last24)
timeit("hourly counts+watermarks, 48h, all channels",hourly_counts)
timeit("24h full text fetch",fullday_text)
timeit("trigram FTS one word, 30 days",fts)
```

Output:

```
rows 13500  insert s 12.21  db MB 126.8
24h index lines (id,time,author,160 chars): p50 1.3 ms, p90 1.4 ms (450, 86741)
hourly counts+watermarks, 48h, all channels: p50 0.6 ms, p90 0.6 ms 235
24h full text fetch: p50 6.0 ms, p90 6.1 ms 665292
trigram FTS one word, 30 days: p50 0.2 ms, p90 0.3 ms 50
```

## 5. Numbers taken from earlier documents

| Number | Source |
|---|---|
| 62 to 446 chat events a day, about 330 tokens a message, 339 named threads | `2026-10-22-side-agent-helper.md`, "Measured volume" |
| Voice runtime `max_iter=5, timeout=30s, tool_timeout=15s`; sequential tool calls; +16,616 tokens for a 50-message page; channel reads 1.9 to 5.7 s quiet, 14 to 20 s busy | `2026-10-22-voice-tool-audit.md` |
| Schema-3 first paint 2.16 MB in 34 requests; adding a day rewrites 387,547 bytes | `py/wrkviz/ARCHITECTURE.md` |
| Model list prices | `2026-10-22-side-agent-helper.md`, "Which model" |

## Not measured

- Model latency for hourly chat rollups and the 24-hour narrative (P6, P7 in appendix 8): estimated.
  `logbook bench` is proposed to measure them on the deployment's gateway.
- The Slack deployment's per-consumer cache miss reasons: inferred from code (appendix 1); the
  diagnostics report holds the counts.
- The voice model's context window and tool reliability: unknown; appendix 9 proposes the harness.
