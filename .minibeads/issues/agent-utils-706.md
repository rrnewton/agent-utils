---
title: 'rust-primary-wrkviz: port wrkviz to Rust behind the same CLI'
status: open
priority: 2
issue_type: task
created_at: 2026-10-30T06:44:30.605209022+00:00
updated_at: 2026-10-30T06:44:42.826876431+00:00
---

# Description

[muse watermelon] rust-primary campaign milestone 5: port wrkviz to Rust incrementally behind the same CLI. Python wrkviz is ~60k lines with thin tests, so the first slice builds a test corpus (fixture session/archive inputs plus golden outputs from the Python edition) before any Rust code. The archive record formats (schema 2 and schema 3) get exactly one Rust type as source of truth with a generated schema the Python mirror validates against. The static site assets (JS/CSS/HTML) carry over unchanged. Port subcommand by subcommand with differential tests; switch primary in validation/editions.json only at parity. Predecessor: #562 rust-primary-wrkslots (slices 2-58 landed; wrkslots audit has no funded Python-owned corners left).

### Slice 1: Rust inspect over a synthetic corpus

- Commit: `5c130a78`, landed on `main`.
- New crate `rs/wrkviz` ports `wrkviz inspect` at differential parity over a deterministic synthetic schema-3 archive (`wrkviz.synthetic`, small scale). The schema-3 bootstrap and shard catalogue records are defined once in Rust (`inspect.rs`) with the Python `_SchemaThreeArchive` acceptance rules sentence for sentence; the schema-1 timeline fallback and the snapshot pointer summary (`snapshot_store.rs`) match, as does the `json.dumps(indent=2, sort_keys=True)` output shape and the `--help`/`--version`/no-args surface. Commands the Rust edition has not ported refuse by name.
- Differential: `py/tests/test_wrkviz_inspect_rust.py` has 10 cases (synthetic counts, missing/empty archive, schema-1 fallback, partly-published decline, unnamed-shard decline, legacy snapshot sizing incl. symlink-to-dir, bad manifest, CLI help/version) and passes in full. Strict mypy and clippy clean.
- Review: one adversarial reviewer found symlinked-directory tree sizing, `line_ranges` on unvalidated streams, schema-version float equality, unreadable teams propagation, the CLI help surface, partial-search list rendering, CPython repr quoting, manifest read order, float rendering, shard field order, and broken-symlink store-root resolution; all fixed before landing.
- Gate 101 (`/tmp/rust-campaign-gate101.log`): every selected step passed except the two wrkslots shards (host-load timeouts; solo 610 + heartbeat flake passing alone, and 611 + hold flake passing alone), one vibe-talk browser step (passes alone), and `rust.agentctl.test` (passes alone, 1815 cases). The inventory steps this slice adds (validation-graph, dispatch-wiring, editions, rust.wrkviz.test, wrkviz shards) pass in the gate itself. Push preceded by a rebase onto upstream commits sharing no files with this slice.
- Remaining named corners: `--userguide` refuses by name; JSON number domain beyond serde_json (arbitrary-precision integers, NaN/Infinity literals); CPython-vs-serde parse-error wording. Next slices: the next read-shaped subcommands (query/serve reads), then writers (ingest/summarize/build), then the edition switch at parity.

### Slice 2: Rust query list over schema 3 and schema 1

- Commit: `d220d25b`, landed on `main`.
- New `rs/wrkviz/src/query.rs` ports `wrkviz query list` for `teams`, `agents`, `phases`, and `rollups` in all four response formats (`json`, `jsonl`, `markdown`, `text`), with `--team`, `--start-time`, `--end-time`, `--kind`, and `--agent` filters. Schema 3 reads spine line ranges with the Python `_SchemaThreeArchive` unwrap and team-stamp rules; schema 1 reads the monolith with sole-team stamping. The query envelope uses Python's canonical `ensure_ascii=False` JSON shape, and the `query` and `query list` help surfaces match the Python edition verbatim. Query actions the Rust edition has not ported (`show`, `search`, `prompts`, `messages`, `stats`) refuse by name.
- Named corners: a schema-2 bootstrap refuses by name (that generation is not yet ported); shard-internal corruption spellings beyond record-kind/team unwrap, CPython-vs-serde JSON parse wording, and the JSON number domain beyond serde_json remain as in slice 1.
- Differential: `py/tests/test_wrkviz_query_list_rust.py` has 13 cases over the deterministic synthetic schema-3 corpus plus a hand-written two-team schema-1 archive (all resources, three non-JSON formats, team/kind/agent/window filters, filter errors, missing archive, CLI help and argparse errors, unported-action refusal) and passes in full; the slice-1 inspect differential (10 cases) still passes. Strict mypy and clippy clean.
- Review: adversarial review attempted; the reviewer subagent died with a runtime error, so the author self-reviewed (validation order, duplicate-reference index, collection caching, option prefixes) before landing.
- Gate 110 (`/tmp/rust-campaign-gate110.log`): 202 steps passed, including every wrkviz step; the failures were the four wrkslots component shards and one lifecycle host shard under host load (each with timing-sensitive audit/lifecycle cases). Every failed step passes solo on the final commit: host shard 115 passed, and wrkslots shards 316/361/329/304 passed in 389/448/437/621 s. Push preceded by rebases onto vibe-talk-only upstream commits sharing no files with this slice.
- Next slices: `query show`, then search/transcript reads and `serve`, then writers (ingest/summarize/build), then the edition switch at parity.

## Slice 3 landed: Rust `query show` (main 9d24a8b8, fmt 602ea10a)
- Ports `query show` for team/agent/phase/rollup references on schema 3 and schema 1: `Timeline::show_record` with `_expand_phase`/`_expand_rollup`, schema-3 `activity_bounds` merge via whole-team refs tables with duplicate detection, schema-1 whole-index lookup, plain/gzip phase details, rollup markdown scoped under `teams/<team>/summaries`. `message:`/`tool:` transcript references refuse by name until the search slice.
- Adversarial review (121 schema-1 probes, 25 mismatches) found and this slice fixes: CPython float spelling plus arbitrary-precision integers in JSON output (`arbitrary_precision` serde feature, `py_float_repr` rewritten), dotfile detail/rollup suffixes (`.json` basename has no suffix), markdown transcript entry validation, and argparse abbreviation/`=value`/multi-extra/`--` forms for `show`. Remaining named wording risks: CPython JSON parse-error text and timestamps beyond i64.
- Differential: `py/tests/test_wrkviz_query_show_rust.py` (6 cases incl. reviewer edges) plus list (13) and inspect (10) suites all pass (29 total); strict mypy and clippy clean.
- Gate 112 (`/tmp/rust-campaign-gate112.log`): 205 passed; `repository.format.rust` fixed by 602ea10a and passes solo (`cargo fmt --check`); `vibe-talk.check.scroll-stability-browser` passes solo on the final tree (133s, `/tmp/rust-campaign-scroll-solo.log`). Push preceded by a rebase onto vibe-talk-only upstream commits sharing no files with this slice.
- Next: `query search/prompts/messages/stats` (transcript projections), then `serve` reads, then writers (ingest/summarize/build), then the edition switch at parity.

## Slice 4a landed: Rust `query stats` (main ce3cf2b4, mypy fix 87aa3c49)

- Ports `archive_stats` / `TranscriptQuery.content_stats` / `TimelineQuery.summary_stats` / `format_stats` to Rust: transcript manifest and sidecar-index validation, per-member gzip inflation with the uncompressed SHA-256 reproduced, prompt authorship classes, response linkage, summary availability and text totals, project overviews, and all four output formats. Adds the chunked transcript reader and the `sha2` dependency. Registered in `validation/components.json`; the editions note records `query stats` at parity.
- Differential: `py/tests/test_wrkviz_query_stats_rust.py` (5 cases) over the synthetic schema-3 corpus (with `extract-transcripts` applied) and a hand-written schema-1 archive with a hand-written transcript export: all formats, team/kind/window filters, integrity refusals (manifest digest, missing transcripts), help and argparse errors. Inspect/list/show suites still pass (34 wrkviz cases total).
- Adversarial review findings fixed before landing: abbreviated long options with `=value`, arbitrary-precision timestamps under windows, CPython `NaN`/`Infinity` acceptance, RFC3339 field validation (leap second refused, offset minute 60 accepted), help/choice precedence and `-hx`/`-h=x` forms, gzip CRC and UTF-8 error spellings, and Python's U+001C-U+001F word splitting. Named remaining gaps, shared with earlier slices: CPython-vs-serde JSON syntax error wording, zlib deflate error text, lone-surrogate strings.
- Gate 114 on the pre-rebase HEAD: 203 passed; failures were the new test's strict-mypy error (fixed by 87aa3c49, passes solo), `rust.agentctl.test` and wrkslots shards (pass solo). Gate 115 after rebasing onto the other team's local-ci-hub series failed on that series' own reds; held until the upstream fix-forwards (79874c97, d7adaf68) landed, then the failed infra steps and all four wrkslots shards pass solo on the final commit (see the #562 note for the audit-shard investigation).
- Next: `query prompts`/`messages` (slice 4b, reusing the chunked reader), then `query search`, `serve` reads, writers, edition switch.

## Slice 4b landed: Rust `query prompts`/`messages` (main dd314ffa)

- Reuses the slice 4a chunked reader with on-demand line parsing, so paging, dense-ordinal ranges, instant spans, and backwards tails never read a defect beyond the returned page, matching the oracle's generators. Authorship selection, response linkage with `prompt_ordinal`, exhaustive `--verify` across all three projections, and the full argparse surface. Differential `py/tests/test_wrkviz_query_prompts_rust.py` (3 cases over the synthetic corpus and a hand archive); 37 wrkviz cases total pass.
- Adversarial review (about 180 comparisons) found and fixed: materialization-vs-laziness exit flips (corrupt line, digest tamper, unsorted tail past a `--limit`; early-member defects on multi-member tails), i64 ceilings on timestamps and ordinals, missing-`--output` precedence, and the oracle's three-pass field validation order in `list_messages`. JSON syntax error wording remains the known cross-slice gap.
- Gate 118: `validate: PARTIAL` with every selected step passed (209 passed, 0 failed); pushed as dd314ffa.
- Next: `query search` (slice 4c, including `message:`/`tool:` show references), then `serve` reads, writers, edition switch.
