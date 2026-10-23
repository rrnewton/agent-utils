# Appendix 3: A feature-complete Rust port of wrkviz on `logbook`

> **Update 2026-10-23 (`#455 vibe-talk-next-plan`).** Part of the master plan,
> [`../2026-10-23-vibe-talk-next/README.md`](../2026-10-23-vibe-talk-next/README.md), which links
> here for the detail of the wrkviz port (its milestones W1 to W3). Terms follow its
> [terminology](../2026-10-23-vibe-talk-next/TERMINOLOGY.md).

Part of `#449 wrkviz-vibe-talk-synthesis`. The port is `rs/wrkviz`, a CLI over the `logbook` crate
(appendix 2). It is done when every row below is either ported or deliberately dropped, and when the
Python and Rust editions produce byte-identical website data from the same archive (the
differential test below). The model for this is `#238 agentctl-rust-only` and its parity spec
(`ai_docs/transient/2026-10-11-agentctl-rust-parity/`): port, prove equivalence, then archive
Python.

## Principles

1. **The archive format is the contract, not the code.** A Rust build must read every archive the
   Python wrote (schemas 1 to 3, summary-cache v1/v2 and naming-cache v3, receipts v2) and must
   write schema 3 and artifacts the Python reader also accepts, until Python is retired.
2. **Paid artifacts are never recomputed by the port.** Cache identity is the exact model payload;
   the Rust stager must produce byte-identical payloads for the same inputs, or every cached summary
   becomes a miss and the next `summarize` pays again (about 52 M input tokens on the measured
   archive). The payload-identity test below guards this.
3. **SQLite is added beside the archive, never instead of it.** `logbook`'s `SqliteStore` becomes
   the working index and cursor store; `raw/team.json`, payloads, artifacts and receipts stay the
   durable, git-friendly record.

## Parity checklist

Status values: **port** (same behaviour, Rust), **port+** (port and generalize for chat), **drop**
(retire, with the reason), **later** (after Python is retired).

### Ingestion

| Python | Behaviour to keep | Plan |
|---|---|---|
| `codex.py` | Newline-complete prefix snapshots; reuse, prefix extension or new descendant only; continuation chains from explicit flags; namespaced continuation ids | port, as `CodexSource` with a `Cursor::Files` |
| `claude.py` | Root JSONL plus `subagents/agent-*.jsonl`; subagent instructions presented as user records reclassified | port, `ClaudeSource` |
| `claude_history.py` | Prompts-only team from `history.jsonl`, covered-session exclusion | port |
| `orc.py` (7,350 lines) | SQLite backup snapshots, append guards over named columns, task-note freeze and tombstones, `--accept-orc-prefix-rewrite` per session with bounded diffs, schema-5 boundary tables | port, last of the sources (largest and most intricate) |
| — | Muse transcripts | **new**: `MuseSource`, once the owner says where Muse keeps them |
| `project_config.py`, `ingest-project` | Exact-schema project config; per-team isolation; failures recorded, run continues | port |
| `snapshot_store.py`, `migrate-snapshots` | Vendor snapshots outside the archive, resolution order, interrupted-migration refusal | port, but see "drop" for whole-file snapshots |
| `payloads.py` | Content-addressed tool payload shards, union merge, prune-tolerant manifest | port |
| `identity.py`, `github_*`, `gh_resolve.py` | Project and repository identity, PR titles | port (GitHub metadata through `gh-paced`) |
| `extract-transcripts` | Provider-neutral prompt/message/system-input projection, monotonic union, authorship rules | port+ (chat messages are a second projection family) |

### Model and normalization

| Python | Plan |
|---|---|
| `model.py`, `model_io.py` | port: `TranscriptMeta`, `AgentRef`, groups side table; `raw/team.json` read and written byte-identically |
| `phases.py` (30-minute UTC phases, stable ids) | port; phase ids must match exactly (they key cached artifacts) |
| `activity_bins.py`, `periods.py` | port+ (the same bins serve chat tallies and the availability bar) |
| `run_stats.py`, `token_usage.py` | port; receipts become rows plus the JSON files |
| `artifacts.py` (work-output artifacts) | port |
| `losslessness.py`, `audit-losslessness` (Codex rules) | port; then extend to Claude and Orc, which are reported uncovered today |

### Summarization

| Python | Plan |
|---|---|
| `summary_registry.py` | port as `Summarizer` impls with the same IDs and versions |
| `summarize.py` `build_summary_prompt` (phase, project-overview, technical-rollup, plain-language-rollup) | port; prompts copied verbatim; payload-identity test |
| `naming.py` (agent lifetime names, bounded path/slug repair) | port |
| `summary_artifacts.py`, `summary_catalog.py` | port: envelope v1, selection by strongest compatible version and context score |
| `claude_backend.py`, `codex.py` runner, `backend_process.py` | port as `ModelBackend` impls; add vibe-talk's Messages API backend (new for wrkviz, and much faster than a CLI per call) |
| Legacy hash schemes (summary-cache v1/v2, naming v3) | port the readers only |
| `glossary-definition` (historical-disabled), `terminology.py`, `glossary_audit.py`, `audit-glossary` | drop the writer path and the audit command; keep reading old projections. Replaced by `logbook` topics (appendix 2) |

### Website and query

| Python | Plan |
|---|---|
| `timeline_v3.py`, `seekable_jsonl.py`, `search_bloom.py`, `search_index.py` | port: the schema-3 writer, the multi-member gzip codec with `.index.jsonl` sidecars, Bloom prefilters |
| `render.py` (Markdown, managed-file inventory, sweeps) | port |
| `multi_team.py` (combined export, namespacing, shared UTC axis) | port+ (chat and transcript teams in one export) |
| `static/app.js`, `timeline-core.js`, `index.html`, `style.css` | **keep as files**; the Rust build copies them like the Python build does. Later: chat-aware rendering (appendix 5) |
| `standalone_server.py`, `server.py`, generated `serve.py` | port the handler rules into a reusable Rust module (gzip sidecars, strong ETags, single `Range`, `If-Range`, no listings, refuse `source_snapshots`), used by `wrkviz serve` and by vibe-talk's Work tab. The generated `serve.py` stays Python so an archive remains self-contained without a binary |
| `query.py` (6,351 lines) and the archive-local `./timeline` | port the CLI surface (`list`, `show`, `search`, `prompts`, `messages`, `stats`) to `wrkviz query`; **keep generating** the dependency-free Python `./timeline` until archives no longer need to be readable on a machine without the binary (owner decision) |
| `archive_gc.py`, `gc` | port |
| `transcript_export.py` | port |
| `synthetic.py` (deterministic large fixtures for the browser suite) | port; it becomes the shared fixture generator for chat as well |
| `inspect`, `quickstart`, `refresh*` | port |

### Drop

- **Schema 1 and schema 2 writers.** Already retired as outputs; the Rust build writes schema 3 only
  and reads 1 and 2 for old archives (the Python reader's order).
- **The glossary-definition summarizer and `audit-glossary`.** Disabled since the glossary was found
  not to be a durable ontology; topics replace it.
- **Whole-file vendor snapshots for append-only JSONL sources**, eventually. With a byte cursor and a
  prefix digest, a Codex or Claude snapshot can be the appended byte ranges rather than a full copy.
  This needs the losslessness gate to pass `--require-lossless` first, because the snapshots are
  what the audit compares against. Orc's SQLite snapshots stay.
- **`migrate-build-state`** once every archive has migrated (it is a one-way move).
- **The CLI-per-job model path as the default.** Measured median latency per phase job is 20 to 24 s
  through a CLI; a direct API call is a few seconds (estimate). The CLI backends stay available
  because some deployments can reach a model only through a coding CLI.

## Migration path for existing archives

1. **Read in place.** `logbook::ArchiveDir` opens a Python-written archive read-only: bootstrap,
   spine, shards, `raw/team.json`, payloads, artifacts, receipts. `wrkviz query` and `wrkviz serve`
   work on it at once. No rewrite.
2. **Index once.** `wrkviz index <archive>` builds the SQLite working store from the archive: items
   from `raw/team.json`, artifact metadata from `summary_data`, receipts, cursors from the source
   manifests. Deterministic, zero tokens, repeatable; deleting the SQLite file loses nothing.
3. **Ingest with Rust from then on.** Cursors from step 2 continue where the Python ingest stopped.
   The first Rust ingest of each team runs in **check mode** first: it re-normalizes from the
   snapshots and compares with `raw/team.json` byte for byte; any difference stops the migration of
   that team and names the records.
4. **Summarize with Rust.** The payload-identity test (below) must be green for the summarizer
   versions in use; then Rust `summarize` hits every cached artifact and pays only for misses.
5. **Build with Rust.** Differential test: Python `build` and Rust `build` on the same archive into
   two directories; `data/timeline-v3.json` and every shard must be byte-identical (they are
   byte-idempotent today, which makes this test possible).
6. **Retire Python** per team family, after a period in which both run (the agentctl precedent).

## Tests that make the port safe

| Test | What it proves |
|---|---|
| Payload identity | For every registered summarizer, the Rust stager's model payload for a fixture unit equals the Python's, byte for byte, so cache keys match |
| Normalization differential | Rust and Python normalize the committed fixtures and a synthetic `ci` archive to identical `raw/team.json` |
| Build differential | Identical schema-3 output from one archive |
| Old-archive readers | Archives in schemas 1, 2, 3 and each legacy cache version open and query |
| Append guards | Truncation, prefix rewrite, symlinked paths and Orc prefix mutations refuse, as today |
| Losslessness | The Codex rule table ported; Claude and Orc tables added |
| Browser suite | The existing Playwright timeline tests run against a Rust-built archive |

## Rough size

The Python is about 52,000 lines, of which `orc.py`, `query.py`, `pipeline.py` and `app.js`
(kept) are the bulk. Excluding the browser app, a faithful port is in the range of 25,000 to 35,000
lines of Rust including tests (estimate, by analogy with the agentctl port, whose Rust edition is
denser than its Python). Sequenced as in the main proposal, Claude and Codex ingest plus
phases and export come first because they unlock the Work tab; Orc and the losslessness extensions
come last.
