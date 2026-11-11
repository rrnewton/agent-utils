# Transcript archive: general excavation, project bucketing, rolling deletion

Status: design for owner decision, 2026-11-07. No product code accompanies this
document; the only experiment run for it was a scratch-HOME Muse retention test
(§2). Companion: the concrete project definitions (real repository lists,
per-project sizes, worked `other` examples) live in a private harness document,
`ai_docs/2026-11-07-transcript-archive-projects.md` in the coordinating
harness repository, because this repository never names its consumers. The two
known consumers are called consumer-a and consumer-b below; those labels are
redactions. Claims are tagged where it matters: **measured** (this host,
2026-11-07), **code** (read at the commit this document lands on), or
**quoted** (an earlier audit document, not re-measured here).

## Phone summary (one screen)

Recommendation: build `wrkviz excavate` — a new, thin, host-wide, text-only
excavation — then delete raw transcripts only through each harness's own
mechanism, behind a per-session checksum manifest. Muse's
`session_retention_days` is real code but **did not fire in any scratch test we
could run**, so it is not yet the rolling-window answer (§2); the archive must
not wait for it. On the measured host the three raw stores total ~224 GiB
while the user+agent speech inside them is roughly 0.1–2.5% of bytes, so a
compressed speech-only archive is plausibly **1–2 GiB where 224 GiB now sits**
(§1, §3). New features go Rust-only, gated on the already-scheduled Rust
primary flip — command parity has in fact landed (25/25), so this honours
"no new Rust features before parity" rather than violating it (§7). P0 is not
a config flip: it is a question to the Muse team (Appendix A), an optional
11 GiB stale-view drop, and resuming the existing archive's stalled fetch (§8).

Owner questions — each answer is one word or a pick:

| # | Question | Pick (recommendation first) |
|---|---|---|
| Q1 | Rolling window: archive-then-delete older than? | **14d** / 7d / 30d |
| Q2 | Drop stale Muse view generations now (~11 GiB, Muse stopped)? | **yes** / no |
| Q3 | New excavation feature lands in? | **rust-only after flip** / both editions |
| Q4 | The dictated repo name is an existing submodule of consumer-b's harness (private doc); count it in consumer-b? | **yes** / no |
| Q5 | Encrypted reasoning blobs in the archive? | **drop** / keep |
| Q6 | Tool activity in the archive? | **names+counts only** / none / full text |
| Q7 | Stop syncing the Muse sessions tree between hosts? | **yes** / no |
| Q8 | Primary project = launch containment before innermost repo (§4)? | **yes** / no |

## 1. The problem, measured

One host (the design host, 2026-11-07; disk 3.5T, 84% used), three harness
stores, all **measured**:

| Store | Size | Sessions/files | Notes |
|---|---|---|---|
| Muse `~/.local/share/muse/sessions` | 56 GiB (58 GiB whole data dir) | 1,035 session dirs; 31,759 `session.jsonl` incl. 30,740 subagent logs | `session.jsonl` bytes 40.8 GB; view caches §2; index DB 11 MB |
| Claude Code `~/.claude/projects` | 19 GiB | 2,700 JSONL (134 mains, 2,566 sidechain/workflow) | largest single file 2.25 GB; median 2.05 MB |
| Codex `~/.codex/sessions` | 117 GiB, **plus** a 32 GiB projected-copy DB | 17,339 rollouts | median 1.87 MB, max 3.18 GB; state DB 407 MiB |

The Muse tree is additionally file-synced between hosts, which both spreads
the cost and manufactures duplicates (536 of 773 Muse sessions on one host
were sync copies of another host's — **quoted** from the 2026-11-06 archive
audit). The existing curated archive's raw mirror is already 228 GB across
projects (**quoted**, §6).

What the bytes are (**measured**, bounded samples, §3 gives method and
caveats): genuine user-prompt + agent-visible-text is 0.06–0.2% of Muse
main-session bytes, 0.36–1.39% of Claude bytes, 0.95–2.48% of tool-heavy Codex
bytes (one text-heavy legacy Codex outlier reached 40%). The rest is tool
output (stored two to four times in every format), hook-run records (≈28–41%
of Muse mains, 28–52% of sampled Claude files), repeated system/context
copies (≈15% weighted in Muse mains; the liaison's "checkpoints 9%" is
refuted — measured ≈1% — and "model-request copies 12%" is right only for one
of two series that together run ≈15%), opaque encrypted reasoning (plaintext
thinking measured at 0 bytes in nearly every sample), and envelope overhead.

Owner's requirement, restated: keep a record of the raw text — user prompts,
agent text replies, coordinator↔subagent messages — and be willing to delete
everything else. That record is two orders of magnitude smaller than the raw
stores. Everything below follows from that ratio.

## 2. Muse-native GC: what exists, what the scratch test proved

### 2.1 The experiment (and its cage)

All retention behaviour below was tested **only** in a scratch environment
(`/tmp`, fresh `HOME`/`XDG_*`, outside any git tree), never against the real
stores; the real `~/.config/muse/settings.json` still has no retention key
(verified after the test), and the scratch tree was deleted. Isolation was
proven, not assumed: under `strace -e trace=openat,...` the scratch processes
made **0** opens of the real session/config stores (35k trace lines), because
Muse honours `HOME`/`XDG_CONFIG_HOME`/`XDG_DATA_HOME` (no `MUSE_HOME`
override exists). Eleven small echo-provider sessions were created in scratch
only; listed before and after; backdating was attempted three ways (file
mtimes + index-DB timestamps; then log-internal `recorded_at` + folder date;
then UUIDv7-embedded times).

### 2.2 Verdict on `session_retention_days`

**Real setting, real whole-session expiry code, no observed firing. Do not
rely on it yet.**

- The key is a genuine user-settings field (**binary strings**); it is absent
  from the real settings, no numeric default is discoverable from files or
  strings, and `muse config validate` rejects it on the enterprise-defaults
  plane. `-c session_retention_days=7` is accepted — but so is
  `="seven"`, silently: unknown keys refuse, wrong-typed values do not.
- Expiry implementation exists (`session_index` maintenance with expiry,
  folder, prune, tick, skew-lock modules; diagnostics like `session deleted
  by retention`, "expired session folder not fully removed; the next process
  sweeps it"). Reading those strings: expiry deletes index row + session
  folder + view sidecar from a periodic maintenance tick in an index-owning
  host. This is whole-session deletion, distinct from compaction checkpoint
  prefix-pruning inside one live log — the two must not be confused.
- Eight trigger attempts (retention unset / 7 / 0 × `exec`, `serve`, 45 s TUI,
  direct binary) deleted **nothing**: no folder, row, log line, or view.
- A valid old session **cannot be fabricated** for test purposes: mtimes are
  not the (sole) clock; DB timestamps are a derived cache that heals from the
  logs; `created` follows the first `recorded_at` but activity/order stayed
  pinned to UUIDv7-embedded start times; shifting the UUIDs broke the hash
  chain and the session indexed as `corrupt` — and was still kept. So the
  expiry clock (created vs last-activity vs order key), the boundary
  (inclusive/exclusive), and behaviour for corrupt/zero-prompt/named sessions
  are **undetermined**. Candidate explanations (first tick needs longer
  uptime; feature gated off in this build; effective value only via the
  launcher's server-TTL path — launcher strings show it can write this key
  from a TTL config and tracks that in a state file that is absent here and
  did not override scratch values; candidates need activity facts the test
  subjects lacked) are listed verbatim in the request to the Muse team
  (Appendix A).
- Independent real-store bound (**measured**): an untouched session from
  2026-09-22 (46 days) survives under daily heavy use. Whatever the default
  is, no ≤46-day GC fires today.
- Live-session safety is **unproven**: strings suggest fencing (prune refused
  for a live task link, per-session lock, skew lock), but nothing fired, so
  nothing was observed. This must be a team-confirmed guarantee before the
  setting is ever enabled for real.
- "Strict" is a red herring: `StrictSessionRetentionUnavailable` and kin name
  *durable sequenced append* ("strict retention") versus `--no-session-log`
  memory-only sessions. They have nothing to do with retention days — but
  matter to an archiver: a memory-only session leaves no log to archive.

### 2.3 What else Muse offers

- No `gc`/`prune`/`cleanup` subcommand exists (`muse gc` falls through to
  root help). `muse sessions --json` lists only *running* sessions.
- `muse export [--session …] [--out …] [--redacted]` is real, offline, and
  never deletes: one self-contained schema-v1 JSON decode of the durable log
  (387 KB from a 253 KB scratch log, lineage and diagnostics included; works
  with the view cache absent — proven in scratch). It is a supported
  per-session extraction primitive, not a bulk archiver; raw mode contains
  secrets exactly as logged.
- The durable store is more than `session.jsonl`: a session-index SQLite DB
  (whose rows keep first-prompt/search text), per-session peer-history
  SQLite (322 files, 5.2 GiB — **measured**), cron DBs, journal/index bins,
  and view sidecars. Hand-deleting session folders under a live index is
  exactly the deep surgery the owner wants to avoid; Muse even has a
  `SessionLogDeleted` error for a live session finding its log gone. §5
  therefore treats Muse bulk deletion as blocked on the team.

### 2.4 The `.msp-view-g*` caches (~11.4 GiB)

These are per-session **materialized views** of `session.jsonl` (projection
journal + index + snapshot + a `HEAD.json` naming the source sequence and
generation) for fast session reads — derived data, not a source. The current
binary (1.4.15) creates only generation g11 (every scratch session got
exactly one g11 dir). Real store, **measured**: v1 105 MB, g6 3.6 G, g7
856 MB, g8 5.2 G, g9 1.4 G, g10 308 MB, g11 63 MB — stale generations
(v1+g6..g10) total ≈11.4 GiB.

Very likely safe to drop, **not proven**: contents are explicitly a
projection (`source_through`, byte counts); strings describe regenerate-at-
first-touch and repair-on-next-open; export was proven with the view renamed
away. Caution: one 30 s scratch resume without a view had not rebuilt it when
killed (slow, lazy, or stuck — inconclusive); a skew lock suggests
multi-version coordination, so a host still running an older binary could
want its own generation; deleting under a live writer is untested. Rule for
the design: never archive from views; dropping stale generations is a P0
*manual, receipted* act (Muse stopped, single-version host, Q2) plus a
standing question to the team — if maintenance already GCs them, why do
11 GiB persist? (Appendix A.)

### 2.5 The other two harnesses' native deletion (for §5)

- **Claude Code**: no age prune, no per-session transcript delete, no export
  command (files are plain JSONL; copy *is* export). `cleanupPeriodDays` is
  not set in any local config (**measured**; any built-in default unverified
  locally). `claude purge [path] [--dry-run]` deletes *all* state for a
  whole project — too coarse for a rolling window. Sessions are independent
  files with no central DB: one session = `<uuid>.jsonl` + `<uuid>/`
  (sidechains, tool-results, workflows) + `file-history/<uuid>/`. Removing
  exactly those three paths for a non-live session corrupts nothing; the
  prompts also persist in `~/.claude/history.jsonl` (35 MB) regardless.
  Never remove a project dir wholesale: `memory/*.md` in it is shared
  project memory, not transcript.
- **Codex**: no age prune and no export command, but a real per-session
  `codex delete <SESSION>` exists — and is the *only* safe deletion path,
  because the rollout file is not the whole session: a 32 GiB thread-history
  DB holds a projected second copy (with byte offsets into the rollouts) and
  the state DB keeps first-message/preview text per thread (**measured**
  schemas, read-only). Raw `rm` would neither free the projected bytes nor
  remove the index text, and would dangle the offsets. A rolling policy is
  therefore: enumerate old thread IDs from the DB (read-only), call
  `codex delete` per ID, receipt each. Whether the 32 GiB DB file itself
  shrinks without a vacuum is unknown — implementation phase must measure it
  (on a scratch copy; no deletion was exercised for this design).

## 3. wrkviz as a general archive: `excavate`

### 3.1 Where wrkviz stands (code, not aspiration)

wrkviz already contains most of an excavator, in both editions with shared
formats (**code**, file references are to the Python tree; Rust mirrors are
named in §7):

- Parsers for every store on a host: Codex rollouts (`codex.py`), Claude
  sessions + sidechains (`claude.py`), Muse session dirs incl. bound
  subagents (`muse.py`), Claude prompt history (`claude_history.py`), Orc
  SQLite (`orc.py`); `.zst`-compressed upstreams decode transparently
  (`compressed_sources.py`).
- The text reduction the owner asks for exists but is downstream and
  **coordinator-only**: `extract-transcripts` (`transcript_export.py`)
  projects `user_prompt`/`assistant_message`/`system_input` events from
  coordinator threads only into compressed JSONL. Subagent speech,
  `inter_agent_message` events, and spawn briefs/results are excluded there
  by design. Ingest itself keeps far more than text (tool text
  payload-detached, encrypted-reasoning markers, lifecycle) plus a verbatim
  vendor snapshot — its centre of gravity is provable losslessness
  (`audit-losslessness`), the opposite of a text-only charter.
- Live-file and idempotence machinery is the strongest part: newline-complete
  prefix reads (a half-written final line is next run's business),
  per-source append-only guards (`copied_bytes` + SHA-256; shrink/rewrite
  refuses by name), write-if-changed canonical output, a monotonic
  never-shrinking transcript union keyed by stable occurrence IDs, seekable
  multi-member gzip with digest sidecars (measured 132.7→28.1 MiB on the
  reference corpus), one writer lock, per-run receipts.
- Selection today is **registration, never discovery**: one ingest = one
  named root lineage; `ingest-project` = a strict schema-v1 manifest naming
  every team (`project_config.py`). The only whole-host enumerator,
  `discover-agents`, sweeps all stores and links native children + agentctl
  lanes + reports cwd orphans — but writes only a report.
- Two deliberate redactions matter later: archived sources null out
  `working_directory` (credentials stripped from repo URLs) for *published*
  archives (`pipeline.py`), and Claude thinking blocks are excluded from the
  durable transcript by an explicit comment. The general archive is private
  and needs the cwd — it is the best project key (§4) — so it must be a
  separate, non-publishable layout, not a relaxation of the published one.

### 3.2 The missing 20%: enumerate, project all speech, stay thin

New subcommand `wrkviz excavate` (the owner's word; the `ingest-*` family
stays team-curated), one run per host, zero model calls:

1. **Enumerate** every store with `discover-agents`' loaders (lift, don't
   rewrite): per provider, find every session file/dir, head-read ID,
   start time, launch cwd evidence (§4.1), parent link. Session key is
   `provider:session-id` everywhere; lineage = root + native children.
   Duplicate copies of one session (sync twins, multi-host mirrors) dedupe
   on session key + content digests, keeping an occurrence list.
2. **Parse** each source's newline-complete prefix with the existing
   parsers. No verbatim snapshot, no payload store, no `team.json` — the
   **thin path**. Per-source watermark manifest instead: `{host, provider,
   session_key, source_relpath, copied_bytes, prefix_sha256, line_count,
   last_seen}`. Append-guard semantics carry over: unchanged size+mtime is
   the fast path; shrink or prefix-hash mismatch is reported by name and the
   already-extracted text survives via the monotonic union (the only loss
   window is a never-yet-seen tail rotated away between runs — bounded by
   running excavation daily against a ≥7-day deletion window, §5).
3. **Project all speech**, every thread, into text records (shape reuses
   `transcript_export`'s base record: schema version, record type, stable
   occurrence + logical IDs, provider, thread/turn, agent path, timestamps,
   text, `content_sha256`, source path+line, native ID, ingress/author
   classification):
   - `prompt` — every genuine user prompt (Claude: string/text user records,
     not the tool-result majority; Codex: role-user `input_text`; Muse:
     run-started prompt, **canonical copy only** — all three formats store
     the prompt 2–3 times; keep one, note the copies dropped);
   - `response` — committed assistant text, linked to its prompt where the
     existing linker applies;
   - `inter_agent` — attributed inter-agent messages (Codex `agent_message`
     natively; elsewhere, prompts/responses whose sender is recovered from
     the `[from NAME via agentctl …]` header parsed at extraction —
     **measured**: ≈9.9k Claude history lines and ≈4.4k Codex history lines
     carry that literal, an upper bound since quotes of headers count too);
   - `spawn` / `result` — delegation briefs and subagent results as edges
     with text (the parsers already lift these);
   - `system_input` — runtime notices fed to the agent (kept: they are text
     the agent acted on; small).
   Tool inputs/outputs, hook records, context copies, traces, and encrypted
   reasoning blobs are **not** archived (Q5/Q6; Q6's compromise — tool name
   + timestamp + byte-size skeleton records — costs ~nothing and preserves
   the rhythm of a session without its bulk; it is a record type, off/on by
   flag, not new storage). Dropped-duplicate accounting (copies seen per
   kept record) goes in the run receipt, so "what did we throw away" is
   always answerable in counts even after the bytes are gone.
4. **Agentctl queues** are a fifth, later provider (loaders already exist in
   discover/responsiveness): they alone hold undelivered/failed prompts and
   exact queue times. v1 gets coordinator↔subagent speech from harness logs
   + header parsing; the queue provider must then dedupe against the same
   text appearing harness-side (the logical-ID scheme will *not* merge
   those automatically — matching is a stated subproblem, discover's
   prompt-key heuristic is the seed).

Thin vs fat, decided explicitly. The fat path (run today's ingest per
lineage) retains verbatim snapshots — typically ⅔ of archive bytes — plus
payloads and a full re-parse per run; applied host-wide it roughly *doubles*
the disk problem it is meant to solve (the existing archive's precedent:
ingesting ~4,750 registered lineages was estimated at 150–250 GiB more
snapshot disk, **quoted**). The thin path forfeits re-projection if
classification rules change and cannot resurrect a rotated never-seen tail.
For an archive whose charter is *speech only*, thin is the honest shape; fat
remains available per-project through the existing `ingest-*`/`extract`
flow for curated, publishable archives. One shared piece must grow a
text-only rule table either way: `audit-losslessness` as written will
correctly condemn a thin archive, so snapshot-deletion blessing needs a
text-charter variant (§5's manifest is that variant's input).

Scale notes (**code**): per-source failure collection (one corrupt or
rewritten session must never block a host run — `ingest-project`'s
collect-failures shape, applied per source); the monotonic baseline must be
sharded per source/project (today's single `occurrences.jsonl` loaded whole
becomes the hot spot at host scope); grouping is by lineage only for
lineage facts — the archive unit is the **session**, not a wrkviz team, so
the known team-explosion failure (2,549 one-shot automation roots registered
as teams, **quoted**) does not recur: one-shot `exec` sessions are archived
as sessions, tagged by kind, never promoted to teams.

### 3.3 Layout

Private, per-archive-root; growing collections each own a directory (house
rule):

```
<archive>/
  projects.json            # copy/pointer to the private project defs (§4) + its sha256
  text/<project>/<provider>/YYYY-MM.jsonl.gz        (+ .index.jsonl sidecars)
  manifests/sessions/<project>.jsonl.gz            # one line per session (§5 gate input)
  manifests/sources/<host>.jsonl.gz                # per-source watermarks
  xrefs/<project>.jsonl.gz                         # sessions touching project, primary elsewhere (§4)
  runs/<run-id>.json                               # receipts: counts, digests, failures, drops
  .wrkviz.lock
```

`<project>` includes `other` (further split in records by cwd, not by
directory — cwd cardinality is unbounded). Shard key = session start month,
so a rolling run rewrites only the current month and the months it touches;
records sort by (timestamp, occurrence id) inside a shard; seekable-gzip
sidecars give `query`-style random access later. The corpus is plain JSONL
under `gunzip -c` — no wrkviz required to read it in 2030.

### 3.4 Size estimate, with method

Text fractions are **measured** on size-percentile samples per format
(decoded field bytes ÷ file bytes; duplicates counted where stored; this
host is automation-heavy — e.g. Codex threads 12,490 subagent vs 2,828
user — so fractions run far below a human-chat corpus; treat as
per-session-type ranges, and the store-wide products below as
order-of-magnitude extrapolations, not measurements):

| Store (raw) | Speech fraction (samples) | Speech, uncompressed | After gzip ≈4.7× |
|---|---|---|---|
| Claude 18.7 GB JSONL | 0.36–1.39% | ≈75–260 MB | ≈16–55 MB |
| Codex 125.6 GB rollouts | paginated 0.95–2.48%; legacy outlier 40% | low single-digit GB (upper order) | ≲1 GB |
| Muse 40.8 GB session.jsonl | mains 0.06–0.2%; brief-heavy subs higher | a few hundred MB | ≲100 MB |

Adding spawn/result/system-input text and the §3.2 skeleton does not change
the order. Working estimate: **the whole host's retained speech compresses
to roughly 1–2 GiB**, against ~224 GiB raw (and against 83 MB for the two
prompt-history files alone, which already exist and excavation subsumes).
The first full run replaces this estimate with a receipt.

## 4. Project bucketing

### 4.1 What evidence a session actually carries

**Measured/code**, per provider — and the headline is *launch-time only*:

- Muse: one `route_facts.cwd` record per session (1 occurrence in a
  19,894-line session) + `workspace_root` in the opening records, branch
  observations with commit; **no git-remote key anywhere**. wrkviz reads
  head-only; mid-session cwd change is metadata-invisible.
- Claude: `cwd` + `gitBranch` + `sessionId` on essentially every record —
  per-record in the bytes, but current readers take the head value only. No
  commit hash, no repo URL; the project dir slug encodes the launch cwd.
- Codex: the richest — `session_meta{cwd, git{branch, commit_hash,
  repository_url}}`, per-turn `turn_context.cwd`, per-command cwd; wrkviz
  keeps launch `cwd` + canonical `repository_url` and discards per-turn cwd.
  Codex is the only provider recording a git remote at all.
- Outside the transcripts: agentctl records link agent→native session
  (+ launch cwd; the registry's own location identifies the harness — slot
  reference fields exist but were null in every sampled record, so joins go
  through cwd); wrkslots ACTIVE/ARCHIVED rows map slot-dir → checkout repo
  name (remote URL only as a hash) → agent name, surviving slot deletion.

Consequence: per-turn project attribution is a *new extraction* (Codex
`turn_context`, Claude per-record cwd — cheap; Muse tool `workdir` fields —
payload mining, later), not something recoverable from today's normalized
data, whose published form redacts cwd entirely (§3.1, §6.3).

### 4.2 Project definitions: one private config file

The first repo-set project registry this fleet has had (none exists today —
**code**: the closest shapes are the archive manifest's
`projects[{label, repository_url}]` list and wrkslots checkout names).
Strict JSON in the `project_config.py` style, versioned, hashed into every
run receipt; lives with the private archive/harness, never in this repo:

```json
{
  "schema_version": 1,
  "projects": [
    {"slug": "agent-utils",
     "repositories": ["https://github.com/example/agent-utils.git",
                       "https://github.com/example/dev-agent-utils.git"],
     "cwd_roots": ["/home/user/work/dev-agent-utils"]},
    {"slug": "consumer-b",
     "repositories": ["https://github.com/example/consumer-b-harness.git",
                       "https://github.com/example/consumer-b-core.git"],
     "cwd_roots": ["/home/user/work/consumer-b-harness"]}
  ],
  "aliases": {"https://old.example/x.git": "https://github.com/example/x.git"},
  "overrides": {"include": {"consumer-b": ["codex:<session-id>"]},
                "exclude": ["muse:<session-id>"]}
}
```

`repositories` are canonical URLs (existing canonicalizer); `cwd_roots` are
containment areas whose project outranks an innermost vendored repo (§4.3);
`aliases` absorb renames/transfers/forks; `overrides` are the auditable
escape hatch the existing classifier already pioneered (`{"include":
["codex:ID"]}`). The concrete lists are in the private companion document.

### 4.3 The assignment rule

Unit: a **lineage** (root + native children) gets exactly **one primary
project**, decided from the *root* session; every other project it touches
gets a cross-reference (§4.4), never a second copy of the text. Evidence is
consulted in strict order; first match decides, and the session manifest
records which layer fired with the matched value:

1. **Explicit override** by `provider:session-id` in the config.
2. **Agent identity**: agentctl `native_session` → agent record → the
   registry (harness) it lives in; refine with launch cwd → wrkslots slot →
   checkout repo. (Strongest for our fleet: it resolves the observed case
   of a consumer-b lane whose cwd was an agent-utils slot — registry says
   consumer-b, outranking the cwd layers below.)
3. **Recorded git remote**: Codex `repository_url` (canonical, via aliases)
   → `repositories`. If the remote names a repo vendored inside a known
   `cwd_roots` area, layer 4's containment still wins for *primary* (below);
   the remote repo is recorded as `repo` + cross-reference.
4. **Launch-cwd containment, longest match**: cwd inside a project's
   `cwd_roots` (harness roots and their slot subtrees; submodule paths
   resolve to the *containing* project for primary, innermost git repo for
   the `repo` field). A cwd inside a bare checkout of a listed repository
   (no harness around it) assigns by that repo.
5. **Content signals** (first prompt names a repo; mention counts over
   thresholds) — the existing classifier's fallbacks, worth ~7% of its
   assignments (**quoted**): these never assign outright; they produce a
   **borderline** report entry, primary stays `other` until an override
   promotes it.
6. **`other`** — keyed in records by sanitized launch cwd
   (`other:/home/user`, `other:/tmp/…`). Machine-repair and scratch sessions
   live here, honestly.

The load-bearing choice is *containment before innermost repo* for primary
(Q8). It answers the owner's question directly: consumer-b work that dives
into a vendored agent-utils checkout is consumer-b's session (that is whose
work it was), with agent-utils cross-referenced as the repo touched; a
session launched from an agent-utils slot to fix agent-utils is
agent-utils' even if a consumer-b need prompted it — launch context is the
observable, intent is not, and overrides exist for the exceptions that
matter. Project answers *whose work*; `repo` answers *what code*.

Edge cases, each with its fate: children inherit the root's primary
unconditionally (a child in a foreign slot still cross-references it);
harness-root coordinators take their harness's project, correctly, because
the lineage is the unit; sync/mirror duplicate sessions dedupe before
assignment (one manifest entry, many occurrences); resumed/continued
sessions join the original lineage; forked sessions' imported history
dedupes via logical records; sessions with no cwd anywhere → `other`;
one-shot automation roots are archived and assigned like everything else
(cheap at text size) but never become teams; two projects listing the same
repository is a **config error**, refused at load (a repo can be vendored by
many, owned by one — contention is exactly what layers 2/4 disambiguate per
session); a repository that is both standalone and a submodule of another
project is assigned per session by containment, not globally — the config
must not try to settle it once for all.

### 4.4 Cross-references instead of duplication

Every session manifest carries `project` (primary), `repo` (innermost,
when known), `repos_touched` (v1: launch evidence only; v2 adds distinct
per-turn cwds where the format has them), and the evidence trail.
`xrefs/<project>.jsonl.gz` indexes session keys whose `repo` or
`repos_touched` intersects the project but whose primary is elsewhere — so
"show me everything that touched agent-utils" works without storing any
text twice. Re-bucketing later (config change, override, better evidence)
is manifest surgery: IDs are stable, affected month shards are rewritten by
a compaction pass, receipts name every move. Assignment is stamped with
config hash + rule version, and recomputed only for new/grown sessions.

## 5. The rolling window: archive, verify, then let the harness delete

Two clocks, kept separate: the **archive watermark** (per source: excavated
through `copied_bytes` at `prefix_sha256`) and **delete eligibility** (per
session). A session is eligible only when *all* hold:

1. **Archived**: a session-manifest entry exists —
   `{session_key, project, repo, host(s), source_files[{relpath,
   complete_bytes, sha256, line_count}], extracted_records, text_bytes,
   digest_root, first/last activity, excavate_run, classification_version}`
   where `digest_root` is the SHA-256 over the session's sorted record
   `content_sha256` values. This manifest *is* the text-charter variant of
   the losslessness audit: it proves what speech was taken, from exactly
   which source bytes.
2. **Verified, twice**: a verify pass re-inflates the session's shard
   members (seekable sidecar digests first, full reproduction of
   `digest_root` second — the existing `query --verify` shape), and the
   session must verify in **two consecutive runs at least 24 h apart** (a
   parser bug then has a day to be noticed before it can orphan the only
   copy). Sources grown since the watermark go back through excavation
   first; verification is against the watermarked prefix, never a fresher
   unarchived tail.
3. **Old**: last activity (max record timestamp as parsed/indexed, *not*
   file mtime — resuming a session refreshes mtime and would keep fossils
   young forever) older than the window (Q1; recommend 14 d to start,
   excavation daily, deletion weekly).
4. **Not live**: Muse — absent from `muse sessions --json` (running
   registry) and past a quiet period; Claude — no live pid-registry entry;
   Codex — no writer lock and quiet. A session still live at window age is
   simply reconsidered next pass; a session that resumes *after* archiving
   just grows a new watermark — deletion always re-checks 1–4 at apply time.

Deletion itself is per-harness, never cross-harness surgery (§2.5):

- **Claude**: remove exactly the three paths of §2.5 per eligible session
  (optionally via a trash dir with its own short TTL for the first months),
  one receipt line per session (paths, bytes freed, manifest digest).
  `history.jsonl` prompts remain — 35 MB, already prompt-only, ignored.
- **Codex**: `codex delete <id>` per eligible thread (enumerated read-only
  from the state DB), receipt per ID, then measure DB shrinkage; a vacuum
  step is added only if measurement proves the file does not shrink (and
  only with the daemon stopped, if ever).
- **Muse**: **blocked on the Muse team** (§2.2, Appendix A). The design
  activates in order: (a) if the team confirms a trigger and clock for
  `session_retention_days`, re-run the scratch experiment — this time
  demanding observed deletion of a genuinely old valid session (the team
  may need to bless a fabrication method, since hash chains defeat ours),
  then set retention to window+margin in real settings as its own
  reviewed step; (b) if the team ships an explicit maintenance/dry-run
  command, prefer it over ambient GC either way; (c) until then, Muse
  relief is only the non-log items: stale view generations (Q2, §2.4),
  tracing growth (1.5 GiB — team question), and stopping the cross-host
  sync of the sessions tree (Q7). Hand-deleting Muse session folders or
  index rows is explicitly out of design.

Failure posture throughout: plan/apply are separate (`excavate
--plan-delete` prints IDs + bytes and writes nothing; apply consumes a
named plan); a failed delete is receipted and never retried silently; any
source that shrank, rewrote, or vanished since archiving drops out of the
plan by name. Nothing in this section runs before §3's archive has proven
itself on read-back (§8, P1 exit criteria).

## 6. Folding in the existing big archive

### 6.1 What exists

The curated archive (≈`agent_logs_archive`, a private repo + rsync mirror;
live copy on another host, an older 63 GiB snapshot measured on this one)
has three layers with no project dimension until the last: fetch mirrors
whole harness trees per host (`log_roots.tsv` — note the template visible
in this repo lacks a Muse root the live copy evidently grew, **quoted**);
registration selects *teams* into `projects/<project>.json`; an
archive-local classifier (not in this repo) auto-registered consumer-b
from 62 to 4,803 teams on 2026-11-03 (cwd first at ~93%, then first-prompt /
content / Codex-remote / registry signals; 379 borderline, 3,764 excluded,
25,589 deduplicated sessions — **quoted**). Built output holds only the
original 64 teams; the 4,747 auto-registered were never ingested (the
150–250 GiB bill was the reason to pause); raw logs are 213.5 GB for
consumer-b of 228 GB total; the last fetch completed 2026-11-03 and one
host's sessions were never classified (536 sync duplicates pending
dedupe) — all **quoted** from the 2026-11-03/06 audits in the private
companion corpus. `summary/<project>` + sibling `.sources`/`.build` stores
are the per-project wrkviz layout; sibling-store pointer files mean the
small published tree can move without the 29 GB snapshot store.

### 6.2 Migration: re-point, then thin-excavate; never re-ingest to re-bucket

Resumable and verifiable at every step; nothing is deleted until the last:

0. **Fetch + dedupe first** (existing tooling, on the archive host): resume
   fetch, drop sync duplicates by session key. No bucketing change is
   measurable before this; it is P0 work (§8).
1. **Freeze** the current layout read-only. Stand up the §3.3 layout beside
   it under one archive root with per-project shards for *every* project,
   not just consumer-b.
2. **Seed assignments from the classifier's own state**: its per-session
   verdict TSV + fact cache (the single most valuable migration asset: 25k
   verdicts with reasons, incremental re-runs in seconds) import as layer-1
   explicit assignments — nobody reclassifies 228 GB from scratch, and
   borderline/quarantine carry over as borderline/quarantine. Rules whose
   inputs are a subset of the cached facts (cwd, host, kind, linkage,
   mention counts) can be re-evaluated from the TSV alone.
3. **Thin-excavate the raw mirror in place**: every mirrored host tree is
   just another "host" to `excavate` (the fetch layer already normalized
   the layout). For the 64 built teams, corpus derivation can start from
   the existing `.sources` snapshots; for the other ~25k sessions,
   excavate the raw logs directly, *thin* — no new snapshots, so the
   150–250 GiB ingest bill is simply never paid. Checkpoint = the session
   manifest; a run resumes where the last stopped; per-source failures are
   collected, never fatal.
4. **Verify before trust**: session counts reconcile three ways (mirror
   enumeration = manifests + named failures; classifier TSV rows ⊆
   manifests; 64 built teams' coordinator speech ⊆ new corpus — noting the
   new corpus is a *superset*, since extract was coordinator-only), plus
   digest-root verification (§5) over the whole corpus, plus spot reads.
5. **Cut over consumers** (summarize/query flows read the new corpus where
   they read extracted transcripts today; curated `ingest-*` teams keep
   working untouched for publishable timelines), run both layouts through
   two weekly cycles, then — and only then, with owner ack — retire the
   old raw mirror per §5's per-harness rules. The mirror's git history and
   the classifier state are kept regardless: they are the audit trail.

### 6.3 What cannot be reused (named traps)

Cwd-based re-bucketing *from the built archive alone* is impossible: the
published form redacts `working_directory` (§3.1) and only Codex retains a
canonical repo URL — hence step 3 reads raw heads (bounded, cheap) or the
classifier TSV, never the built site. Below-team splits (one child to a
different project than its root) are re-ingests in the old model and stay
impossible in the new one by design (lineage unit, §4.3); per-turn
attribution would be a schema change, not a migration. And the migration
must not process the 2,549 one-shot automation roots as teams on the way
through — as sessions they cost kilobytes of text; as teams they were most
of an earlier five-figure summarize quote (**quoted**).

## 7. Python vs Rust

Facts at the landing base of this document (**code**): both editions expose
the same 25 subcommands and Rust dispatches **all 25** (`rs/wrkviz/src/
cli.rs`), including the schema-2 reader and userguide-parity fix that the
rust-primary plan had scheduled for 11-10/11-11 — both already landed.
Remaining gaps are named input-corner divergences (JSON error wording,
non-finite constants, regex features, zlib/SQLite stamp bytes), not missing
commands. Rust correctness rides on 29 Python-driven differential files
(196 tests, static count) plus 896 Python-native tests that are the
behaviour specification; Python is still the router default
(`validation/editions.tsv`: `wrkviz python`), and the flip to Rust primary
(planned 2026-11-13) is one coordinator-approved editions edit + soak — no
machinery is missing. "Python archived 2026-11-20" is a plan aspiration,
not a scheduled fact: Python is the differential *oracle* and owns assets
the Rust build emits from; archiving it first requires frozen goldens,
which nobody has built. (One stale note to fix along the way:
`validation/editions.json` still describes pre-`5af15436` wrkviz.)

Recommendation (Q3): **design now; write `excavate` Rust-only, gated on the
flip actually landing.** Rationale: (a) the owner's instinct — no new Rust
features before parity — is *satisfied*, because command parity has landed;
what remains is a default switch measured in days, and the gate costs the
design nothing since P0 (team questions, fetch resumption, view drop)
needs no new code. (b) Writing both editions now pays the full mirror tax
twice for a primary scheduled to change within the week: estimated per side
(**code** survey §3) ≈1.2–2k lines Python *and* ≈1.8–3k lines Rust
+300–500 lines/side of tests, versus Rust-only ≈1.8–3k + tests. (c)
Writing Python-only now strands the flagship new feature in the outgoing
primary. The oracle problem for a Rust-only command is handled at the
parser seam: equivalence fixtures are generated *from the Python parsers'*
outputs (they are shared facts in the `agent-harness` crate's domain) and
frozen as goldens — no Python `excavate` twin is maintained. Fallback, if
the flip slips more than ~a week past 11-13: implement Python-first with
the usual differential tax, since house rules require parity for a shared
command while Python is primary. Either way, formats stay shared
(seekable JSONL both sides), so existing archives remain readable by both
binaries throughout.

## 8. Options, recommendation, phased plan

Options considered: **(A)** rely on Muse retention alone — rejected:
unproven (§2), Muse-only, and keeps no text record. **(B)** fat
generalization of today's ingest — rejected: snapshots double the disease
(§3.2). **(C)** thin `excavate` + manifest-gated harness-native deletion —
**recommended**. **(D)** scripts outside wrkviz — rejected: parsers,
append guards, seekable writer, identity/dedup logic already exist twice
in-tree; a third copy rots. **(E)** manual purges forever — rejected: 84%
disk and sync-amplified growth say otherwise.

Phases (smallest useful first; estimates are lane-days of agent work, ±50%):

- **P0 — no new features (hours, this week).** Send Appendix A to the Muse
  team. Owner picks Q1–Q8. On yes: drop stale `.msp-view` generations
  (receipted, Muse stopped; ~11 GiB), stop syncing the Muse sessions
  tree (Q7), resume archive fetch + dedupe (§6.2 step 0). Exit: team reply
  or timeout noted; free-space delta receipted.
- **P1 — `excavate` v1 (Rust-only post-flip; ≈3–5 days).** Enumeration,
  thin parse, all-speech projection, watermarks, session manifests,
  bucketing config v1 (+ `other`), §3.3 layout, verify mode. Read-only
  against sources. Exit criteria: full-host run over all three stores;
  record counts reconcile against independent samples (prompt-history
  files, `muse export` on samples); digest verification green; receipt
  replaces §3.4's estimate with a measurement; no source file modified
  (watermark audit).
- **P2 — rolling deletion (≈2–3 days + team wait).** `--plan-delete`/apply
  with the §5 gate for Claude (file triplets) and Codex (`codex delete`
  wrapper + shrinkage measurement); Muse activates only per §5(a/b) after
  a scratch re-test *observes* deletion. Exit: first window's bytes freed
  with receipts; a deliberately corrupted fixture archive refused by name.
- **P3 — existing-archive migration (≈1–2 days tooling + IO-bound runs).**
  §6.2 steps 1–5, including classifier-TSV import and multi-host manifest
  merge (by session key). Exit: three-way count reconciliation; old layout
  retired only after two clean weekly cycles.
- **P4 — polish (as wanted).** Agentctl-queue provider + dedupe; per-turn
  `repos_touched`; corpus query/search surface; scheduling (daily
  excavate, weekly delete); summarize pipelines reading the corpus.

Unverified items this design knowingly carries: Muse retention trigger,
clock, default, live-safety (Appendix A); stale-view drop safety beyond
§2.4's evidence; Codex DB shrinkage behaviour; the live archive's current
fetch config for Muse roots (observed only via audits + the older snapshot);
whether Claude/Codex ever repeat a changed cwd mid-session (immaterial to
v1, which reads launch evidence); dialogue-fraction extrapolation across
session types (§3.4). Each has a named exit criterion or question above;
none blocks P0/P1.

## Appendix A: draft request to the Muse team (send as-is)

> Subject: session_retention_days — trigger, clock, default? (56 GiB store, no GC observed)
>
> We run Muse heavily on shared devservers; one host's
> `~/.local/share/muse/sessions` is 56 GiB (1,035 sessions; an untouched
> 46-day-old session survives daily use). We want a rolling
> archive-then-delete window and would strongly prefer Muse's own
> mechanism over hand-deleting inside your store. In a scratch
> HOME/XDG environment (isolation strace-verified, real store untouched)
> on 1.4.15 we found `session_retention_days` in the settings fields and
> whole-session expiry machinery in strings, but could not get anything
> to delete: retention unset/7/0 × exec/serve/TUI/direct binary — nothing.
> Backdating couldn't fabricate a valid old session (DB times heal from
> logs; activity seems pinned to UUIDv7-embedded times; shifting IDs
> yields `corrupt`, which is kept). Questions:
> 1. What runs expiry, in which process/mode, on what cadence (min
>    uptime, tick interval), and is it gated in this build?
> 2. Age from created, last-activity, or order key? Derived from
>    `recorded_at`, UUIDv7 IDs, folder date, or view journal? Boundary
>    inclusive? Behaviour for `corrupt` rows, NULL activity, zero-prompt
>    bootstrap sessions, subagent children, named sessions?
> 3. `session_retention_days` contract: type/range (="seven" is silently
>    accepted today), 0 = disabled?, default when absent, precedence vs
>    the launcher TTL path (`meta/session_retention_state.json`)?
>    Could load/`-c` hard-fail wrong types, and could `muse config`
>    surface the effective value?
> 4. Guarantees that running/leased/attached sessions (and the current
>    one) can never expire mid-flight?
> 5. Delete ordering/atomicity (row vs folder vs sidecar; the
>    next-process sweep), and could there be an export-before-delete or
>    trash/grace hook?
> 6. Ask: an explicit `muse maintenance --dry-run` listing expiry
>    candidates + bytes, and one greppable log line per expiry decision
>    (session, age, clock, bytes) at default verbosity.
> 7. `.msp-view-g*`: current builds write only g11 here, yet v1+g6..g10
>    hold ≈11.4 GiB — are old generations ever read, does maintenance GC
>    them/orphan views, and is deleting them with Muse stopped safe?
>    (Also: `local-tracing` is 1.5 GiB; dozens of `settings.json.tmp.*`
>    in config.)
> Happy to share the scratch repro (echo-provider sessions, strace +
> before/after listings). Even "not wired up yet, use X instead" unblocks
> us — what we can't do is guess.

## Appendix B: evidence map (for reviewers)

- Muse GC scratch experiment: 11 echo sessions, strace isolation (0 real
  opens), 8 trigger attempts, 3 backdating phases; verdict §2. Real-tree
  sizes and the 46-day survivor: read-only `du`/listings, same date.
- wrkviz survey (both editions, landing base): command tables, data model,
  idempotence, scoping, parity/test census, gap analysis — §3.1, §7.
- Bucketing survey: archive layout (63 GiB local snapshot measured; live
  numbers quoted from the 2026-11-03 classification and 2026-11-06 audit
  documents in the private harness corpus), classifier signals, per-session
  evidence, registry joins, cluster inventory from local git remotes at
  depth ≤2 — §4, §6. Concrete repo lists: private companion only.
- Format surveys: size-percentile samples per store (keys/types/byte
  buckets only; no transcript text recorded anywhere in the evidence),
  text-fraction tables, deletion-safety schemas — §1, §2.5, §3.4, §4.1.



