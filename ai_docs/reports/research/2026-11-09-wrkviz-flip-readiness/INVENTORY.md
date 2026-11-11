# wrkviz Rust-default flip readiness (A3, decision due 2026-11-13)

Lane: mu-rsctl (slot rsctl). Base: `f43d3018` plus landed
`30407c6e` (coordinator agent scope + summarize-project). Issue:
#706 rust-primary-wrkviz. This inventory is the evidence list for the
coordinator/owner flip decision; it does not flip anything.

## Command surface: complete

Dispatch comparison (Python `py/wrkviz/cli.py` subparsers vs Rust
`rs/wrkviz/src/cli.rs` match arms, scripted 2026-11-09): every
top-level command exists in both (quickstart, ingest family,
summarize, summarize-project, build, extract-transcripts, export,
refresh family, github-metadata, serve, inspect, gc,
migrate-build-state, migrate-snapshots, audit-glossary,
audit-losslessness, query, responsiveness, discover-agents), and
every `query` action (list, show, search, prompts, messages, stats)
has a Rust handler. Unknown commands/actions refuse by name with
exit 2 in both editions.

## Recently closed flip blockers

- Schema-2 timeline reader for query/inspect: landed (`5af15436`,
  on main via `00175c6e`), with `py/tests/test_wrkviz_query_schema2_rust.py`.
- `--userguide` subcommand-argument validation: landed in the same commit.
- `summarize --agent-scope coordinator` and the whole
  `summarize-project` command: landed `30407c6e`, differentially
  byte-identical on a scrubbed root+spawned-child tree, including
  the exact invocation the W45 coordinator weeklies use (per-team
  summarize defaults muse/`muse-spark-1.3-internal`, project
  rollup model `watermelon-full-preview`).

## Open items to close or explicitly accept at the flip

1. **extract-transcripts and schema 2 — resolved 2026-11-09.** The
   editions note's parenthetical ("schema 3 and schema 1; the
   schema-2 generation is not yet ported") described the *query*
   timeline reader, ported since (`5af15436`, differentials in
   `py/tests/test_wrkviz_query_schema2_rust.py`). Extract itself
   reads ingested teams, not timeline generations: run against the
   schema-2-only fixture archive on 2026-11-09, both editions refuse
   byte-identically ("no ingested teams found", exit 2), and the
   extract differential on ingested archives already passes in the
   gate. No port work remains here; the flip commit still rewrites
   the stale note (item 3).
2. **Accepted gaps** (from the 2026-11-07 inventory, still accurate):
   non-finite and oversized-integer JSON, lone surrogates, Python
   `re` features in search patterns, and codec/parse error wording.
   None falls back silently; each refuses or errors. These need the
   flip decision to accept them as documented differences.
3. **Editions note accuracy.** The `wrkviz` note in
   `validation/editions.json` still describes the pre-A1 surface in
   places (see item 1). The flip commit must rewrite it to the
   post-flip truth in the same change (the editions checker gates it).

## Flip mechanics (the actual change, when approved)

- `validation/editions.json`: `wrkviz` primary `python` -> `rust`,
  mirror edition `python` with state `parity`, `rust_entrypoint`
  set (today null; the Rust binary is `rs/bin/wrkvizd`), note
  rewritten. `wrkvizd` entry stays as the direct Rust entrypoint.
- `common/bin/engine-resolver`: the `WRKVIZ_ENGINE` per-tool block
  changes from opt-in to opt-out (`WRKVIZ_ENGINE=python` must keep
  working for one release; unset means Rust). Its comment block
  ("default flips only by changing validation/editions.json") is
  the contract this change follows.
- Static site assets: already carried by the Rust build
  (`rs/wrkviz/src/build_assets.rs`); no move needed at flip time —
  verified present, not yet differentially compared file-by-file
  against the Python build output on a real archive (part of soak).
- Soak before flip: run one real weekly cycle with
  `WRKVIZ_ENGINE=rust` (the coordinator weekly run is the canary;
  its exact command lines are on record) and compare receipts and
  Markdown against the Python run.

## After the flip

- A4 (planned 2026-11-20): archive `py/wrkviz` after about a week
  clean; the differentials re-point at frozen goldens first.
- Anything the soak turns up becomes a named issue, not a silent
  Python fallback: post-flip, unported corners refuse by name.
