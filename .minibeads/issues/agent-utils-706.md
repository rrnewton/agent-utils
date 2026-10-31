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

## Slice 4c landed: Rust `query search` + `message:`/`tool:` show (main 487a6e97)

- Ports search_v2 (smart/phrase/literal matching and ranking, trigram bloom prefilter, prompt/response linkage context, phase references), the compatibility `--scope` search, `format_search_results`, and `query show` for transcript search references. Schema-2 search corpora refuse by name; archives without a corpus keep the oracle's rebuild refusal. Differential `py/tests/test_wrkviz_query_search_rust.py` (3 cases); the slice-3 show refusal test and the slice-2 unported-action test now assert parity.
- Reviewer fixes: CPython `casefold()` semantics including its shifted excerpt windows, negative-number needles, Python `int()` grammar with unbounded offset spelling, and separator handling in search errors.
- Gate 120: 206 passed; `python.wrkviz.shard-1` (the obsolete refusal test, fixed by fd1c430c), scroll-stability, and wrkslots shard-2 all pass solo on the final commit. Pushed as 487a6e97 + fd1c430c.
- Next: `serve` reads, then writers (ingest/summarize/build), then the edition switch.

## Slice 4d landed: Rust `serve` (main 68db948b, inventory sort 11474e18)

- Ports `wrkviz serve` to Rust (`rs/wrkviz/src/serve.rs`): a loopback static server at parity with `py/wrkviz/server.py` + `standalone_server.py` — `.gz` companions as complete representations (gzip/identity negotiation, bounded inflation), strong SHA-256 etags, single byte ranges over unbounded CPython integer grammar, content-hashed immutable caching, `source_snapshots` refusal, directory redirects/index, symlinked files served the base-class way including `If-Modified-Since`, the CPython request-line grammar (400/414/431/505, HTTP/0.9 body-only answers, one request per connection), CPython error pages, the stderr request log in local time, and `\nwrkviz: stopped` on SIGINT (new `ctrlc` dependency; chrono gains the `clock` feature).
- Adversarial review probed 18 divergences; all fixable ones are fixed (pipelined close semantics, request-body misparse, malformed request lines, >i128 ranges, hashed-name suffix rule, MIME table, symlink IMS, unescaped error methods, log timezone, sequential `--help` precedence). Named deliberate divergences: pre-flight and out-of-range/occupied-port failures print the oracle's message cleanly instead of its interpreter traceback; the `Server` header omits the Python version; responses are buffered whole (the oracle streams) — a performance note, not a correctness one. Unicode-digit `int()` arguments remain the cross-slice known gap.
- Differential `py/tests/test_wrkviz_serve_rust.py` (4 cases: http.client comparison incl. header order/version, raw-socket malformed requests, CLI errors, pre-flight messages); all wrkviz differentials pass (44 cases). Strict mypy and clippy clean.
- Gate 123 (`/tmp/gate123-serve.log`): only `python.wrkslots.shard-0/1` failed (audit differentials under load, unrelated); both pass solo on the final commit (`/tmp/gate123-shard0-solo.log`, `/tmp/gate123-shard1-solo.log`, rc=0). Pushed as 68db948b + 11474e18.
- Remaining wrkviz: writers (ingest/summarize/build), then the edition switch at parity. Then parallel-experiment-runner.

## Slice 5a landed: Rust `quickstart` (main 08fb6f24)

- Writers stage opens with the trivial print: `quickstart` at byte parity, including the `WRKVIZ_MODEL` override and the oracle's triple-quoted-string line continuations (which join the step-4 command lines). Differential `py/tests/test_wrkviz_quickstart_rust.py`; 24 probe cases matched (help forms, extras, model env variants). No adversarial sub-review for this constant-print slice.
- Gate 125 (`/tmp/gate125-quickstart.log`): only `rust.agentctl.test` failed (one chat-reply test under load); passes solo on the final commit (`/tmp/gate125-agentctl-solo.log`, rc=0). Pushed as 08fb6f24.
- Writer plan from here: 5b `TeamData` archive reader (team.json + promoted task notes, the load_archived_team validations), 5c the seekable-JSONL writer, 5d the extract projection (grouping, authorship rules, monotonic union, container manifest) behind `extract-transcripts` with its run record and writer lock. Then the ingest normalizers (codex, claude, muse, claude-history, orc), `summarize`, `build`, and the composites (refresh/export/gc/migrate), each gated separately.

## Transcript-reading consolidation (per #710, before slice 5b)

One transcript-reading library: `rs/agent-harness` (landed with #710, commit 22c290d4) owns locating and decoding Claude Code, Codex, and Muse session files into its shared `Event` model (`Prompt`/`Text`/`ToolCall`/`TurnEnd`, `Transcript::locate`, stateful `Decoder`, `Follower`, `decode_all`). Design: `ai_docs/2026-10-30-chat-transcript-extraction.md` section (e), whose table names this campaign directly: the wrkviz Python parsers stay the oracle and the Rust edition consumes the shared event model.

Where wrkviz's Python parsing maps onto it:

- `py/wrkviz/codex.py`, `claude.py`, `muse.py` (the `ingest`, `ingest-claude`, `ingest-muse` normalizers) read raw harness session files into the provider-neutral model (`py/wrkviz/model.py`: Agent, Turn, Event, ToolCall, Edge). Their Rust ports will decode through `agent_harness::transcript("codex"|"claude"|"muse")` decoders rather than re-implementing per-format line parsing, extending `rs/agent-harness` where wrkviz needs more than the current minimal events (turn lifecycle and status, tool results and payloads, thread lineage/subagent edges, token usage, encrypted-content markers). Extensions land in `rs/agent-harness` with its own tests, not as a wrkviz-private fork of the format knowledge.
- `py/wrkviz/orc.py` reads Orc's SQLite store; `rs/agent-harness` has no Orc reader and SQLite is outside its session-file scope, so `ingest-orc` keeps its own reader (noted as an accepted exception, like the schema-2 refusal).
- `py/wrkviz/claude_history.py` (`ingest-claude-history`) reads `history.jsonl` prompt history, a different artifact from session transcripts; it will be assessed against `rs/agent-harness` when that slice comes, and any shared prompt-line decoding moves into the crate then.
- `extract-transcripts` (slice 5b), the query/inspect/serve slices, and the archive writers read wrkviz's own normalized archive formats (`raw/team.json`, the seekable corpus), never raw harness files, so they do not touch `rs/agent-harness`; the consolidation constrains the ingest slices, and no new harness-format parsing will be written in `rs/wrkviz`.

## Slice 5b landed: Rust `extract-transcripts` (main f3b498a4)

- First production writer at differential parity: `TeamData` strict reader (`rs/wrkviz/src/team_data.rs`), build-store resolution (`build_store.rs`), archive writer lock/run provenance (`pipeline.rs`, flock via `fs2`), seekable-gzip writer (`seekable.rs`; flate2 now links the system zlib via `libz-sys`), and the full projection with monotonic union, provenance migration, authorship rules, and previous-generation validation (`transcript_export.rs`). The staged writer plan from the 5a note named this 5b-5d; it landed as one slice because the CLI surface is the unit the differential can see. Transcript reading proper stays with the ingest slices on `rs/agent-harness`, per the mapping note above.
- Adversarial review found and I fixed: previous-manifest `teams` strictness, superseded-plain symlink handling, non-UTF8 replace refusal, 0600 file modes, `BuildLocationError`/`UnicodeDecodeError` skip typing, indexed decoder messages, `query.*.pyc` pruning, `./` output spelling, unbounded rule-sort keys (Python's `2**63` fallback). Named divergences: compressed container bytes follow the zlib generation (the oracle interpreter carries zlib 1.2.8, the Rust edition links the system zlib), so containers compare inflated and cross-edition reruns stay valid; CPython JSON syntax wording inside skip summaries; interpreter tracebacks for unreachable loader failures are not reproduced.
- Differential `py/tests/test_wrkviz_extract_rust.py` (9 cases): fresh + second-run stability, authorship rules + skipped team partiality, team selection/duplicates/slugs, corrupt previous manifest, symlink/cache/mode edge cases, relative output spelling, and cross-edition reruns (Rust on a Python-written archive and back, rc 0 both ways). Multi-MiB corpora chunk identically (3- and 5-member containers probed).
- Gate 128 (`/tmp/gate128-extract.log`): only `python.wrkslots.shard-3` failed (known host-load class, unrelated); passes solo on the final commit (`/tmp/gate128-shard3-solo.log`, rc=0). Pushed as f3b498a4.
- Remaining wrkviz: ingest normalizers (`ingest`, `ingest-claude`, `ingest-muse`, `ingest-claude-history`, `ingest-orc`) decoding via `rs/agent-harness`, `summarize`, `build`, composites (refresh/export/gc/migrate), then the edition switch at parity. Then parallel-experiment-runner.

## Slice 6a landed: Rust `ingest-claude-history` (main 67db6f1e)

- First ingest normalizer at differential parity, built on the shared ingest writer every later ingest reuses: snapshot-store resolve/ensure/write-pointer (`rs/wrkviz/src/snapshot_store.rs`), content-addressed payload store (`payloads.rs`), the full deterministic artifact catalog (`artifacts.rs`, ~1,900 lines incl. shell segmentation, URL identity, produced-commit/push evidence), site identity (`identity.rs`), date windows (`window.rs`), and `write_ingested_team` (`ingest.rs`). History-line decoding (display/timestamp/project/sessionId/pastedContents) landed in `rs/agent-harness/src/history.rs` as that crate's fourth format, per the one-transcript-library rule; wrkviz keeps selection (project regex, covered sessions) and team assembly (one coordinator agent per session).
- Differential `py/tests/test_wrkviz_ingest_history_rust.py` (13 cases, fixture-only): fresh + second run byte-stability, append growth (8 then 5 files changed), identity overrides + date window, requested `--snapshot-root`, legacy in-archive layout, truncate/rewrite and pattern-change refusals, damaged payload shard refusal, selection errors, malformed lines, URL prompts producing artifacts, missing file. All 64 wrkviz Rust differentials pass together.
- Adversarial review (one subagent) found 19 items; fixed before landing: a payload-shard panic on short lines (now the canonical-shape refusal), query/fragment leaking into repository labels, empty-port and backslash URL handling, Unicode `isdigit`/key-length/whitespace semantics, truncating (not rounding) RFC3339 milliseconds, CPython TZPATH and UTF-8 codec wording, casefold keys, and argparse semantics for `--h` ambiguity, `--` separation, and bound conflicts. Accepted named gaps: CPython-vs-serde JSON syntax wording, CPython-vs-Rust regex wording plus Python-`re` features the Rust engine lacks (look-around, backreferences), timestamps beyond `i64`, `~` expansion and non-UTF8 argv (both whole-edition, pre-existing).
- Gate 130 (`/tmp/gate130-ingest-history.log`): 208 steps passed; the only failure was a strict-mypy unused-ignore in the new test file, fixed and re-verified solo on the final commit (`/tmp/gate130-types-final.log` rc=0). Two upstream ai_docs commits (60c6a9da) shared no files with this slice, so the rebased push went without a second full gate.
- Remaining wrkviz ingest: `ingest` (Codex), `ingest-claude`, `ingest-muse`, `ingest-orc` (SQLite exception), `ingest-project`; then `summarize`, `build`, composites; then the edition switch at parity.

## Slice 6b landed: Rust Codex `ingest` (main 1e62f5f3)

- Second ingest normalizer at differential parity on the shared writer from 6a. Rollout record decoding (newline-complete prefix with CPython `bytes.splitlines` boundaries, session metadata, content parts, ISO/numeric timestamps with real calendar validation) landed in `rs/agent-harness/src/rollout.rs`; wrkviz keeps lineage discovery (plain + `.zst` via the `zstd` command, streaming first-line discovery), append-only snapshot copying with a no-symlink component walk, continuation links with boundary revalidation, canonical sub-agent trimming, turn/tool/event assembly, and collaboration-edge derivation (`rs/wrkviz/src/codex.rs`, `compressed.rs`).
- Differential `py/tests/test_wrkviz_ingest_codex_rust.py` (9 cases, fixture-only): fresh + second run, continuation session, upstream-retired rollout stand-in, zstd-compressed rollout, prefix-rewrite refusal, malformed rollout, falsy/empty content, window + identity overrides. All 75 wrkviz Rust differentials pass together.
- Adversarial review (one subagent, 12 findings): fixed snapshot symlink following (lexical absolutization instead of symlink-resolving paths), streaming zstd discovery, empty-content presence, Python falsy fallbacks (ids/status/kind/error/occurred-at), timestamp validation, BOM/NaN rollout dialect, prompt-claim tie order, `ensure_ascii` tool output, cwd-label trimming, null `continuation_sessions`, and the `.recorded` window qualifier. One claim (post-`--` reporting) was verified against the oracle and rejected — both editions print the `--`.
- Gate 132 (`/tmp/gate132-ingest-codex.log`): 206 passed; failures were strict-mypy findings in the new test file (fixed; `repository.check.python-types` passes solo on the final commit) plus `vibe-talk.check.read-aloud-blocks-browser` and `cross.agentctl.differential`, both unrelated host-load flakes passing solo on the final commit (`/tmp/gate132-solo.log` rc=0).
- Remaining wrkviz ingest: `ingest-claude`, `ingest-muse`, `ingest-orc` (SQLite exception), `ingest-project`; then `summarize`, `build`, composites; then the edition switch at parity.

## Slice 6c landed: Rust `ingest-claude` (main 12a3c4c6)

- Third ingest normalizer at differential parity on the shared writer from 6a. Claude session-record facts (newline-complete prefix with CPython `bytes.splitlines` boundaries, strict object records with the lenient `NaN`/`Infinity` spelling, `datetime.fromisoformat`-shaped timestamps with truncating millisecond conversion) landed in `rs/agent-harness/src/claude_records.rs`; wrkviz keeps discovery (plain + `.zst` spellings), append-only snapshotting, per-thread dedup and fork-prefix dropping, input provenance classification, turn assembly, subagent routing, and spawn/message edges (`rs/wrkviz/src/claude.rs`, ~1,900 lines). `parse_lenient` in `rollout.rs` is now shared (`pub(crate)`).
- Differential `py/tests/test_wrkviz_ingest_claude_rust.py` (10 cases, fixture-only): fresh + second run, fork-prefix lineage, missing/malformed/conflicting-duplicate inputs, append-only rewrite refusal, upstream-retired subagent stand-in, window + identity overrides, zstd-compressed subagent, and provenance/timestamp edges (same-millisecond prompts, control-only prompt text, seconds-free timestamps). All 86 wrkviz Rust differentials pass together.
- Adversarial review (one subagent, 19 findings): 17 fixed before landing (first-maximal turn attribution on same-millisecond prompts, CPython `strip()` \x1c-\x1f semantics, seconds-free and offset-seconds timestamps, naive-invalid vs timezone-less labelling, >=24h offset refusal, epoch-zero-via-offset, exact int/float equality in dedup identity, directory glob matches named `agent-*.jsonl`, fork-emptied ancestor refusal, empty role/description fallbacks, unbounded `spawnDepth`, integral-float manifest windows, `-help` cluster). One claim rejected with oracle evidence (huge-int tool payload bytes are already exact via arbitrary precision). Named gaps: `NaN`/`Infinity` constants read as `null`, so tool payload text differs in that corner (`{"x":NaN}` vs `{"x":null}`; exit and structure agree); `~user` expansion remains the previously accepted whole-edition gap.
- Gate 134 (`/tmp/gate134-ingest-claude.log`): 207 passed; `vibe-talk.test.unit` and `python.wrkslots.shard-2` failed in the full run (unrelated host-load class) and pass solo on the final commit (`/tmp/gate134-solo.log` rc=0).
- Remaining wrkviz ingest: `ingest-muse`, `ingest-orc` (SQLite exception), `ingest-project`; then `summarize`, `build`, composites; then the edition switch at parity.

## Slice 6d landed: Rust `ingest-muse` (main 4d28321d, rustdoc fix 038aefac)

- Fourth ingest normalizer at differential parity on the shared writer from 6a. Journal record facts (line records with non-object lines skipped, `recorded_at` microseconds, newline-complete prefix) landed in `rs/agent-harness/src/muse_records.rs`; wrkviz keeps bound-subagent discovery (BFS, flat then nested candidates), append-only snapshotting, run/tool assembly with batch-aligned results, and spawn lineage (`rs/wrkviz/src/muse.rs`). Observer sessions never bound by `child_session_bound` stay out, as in the oracle.
- Differential `py/tests/test_wrkviz_ingest_muse_rust.py` (7 cases, fixture-only): fresh + second run, missing/malformed inputs, append-only rewrite refusal, upstream-retired grandchild stand-in, raw-UTF-8 record text, window + identity overrides. All 92 wrkviz Rust differentials pass together.
- Adversarial review (one subagent, 9 findings): the material one was broader than this slice — CPython `bytes.splitlines` splits only on \n/\r (the wide boundary set belongs to `str.splitlines`), so the record splitters in `rollout.rs`, `claude_records.rs`, and `muse_records.rs` (slices 6a-6c included) cut records at raw UTF-8 continuation bytes; all now split narrowly, with a durable raw-UTF-8 differential case. Also fixed: lenient-parse UTF-8 preservation inside strings, a subagent parent-cycle refusal (the oracle dies with RecursionError), saturating first-token math, journal error-line lookup under \r separators, trailing-slash path spelling, and argparse negative-number values (`-.5`). Integers beyond i64 remain the standing named gap. One own-gate catch: the new module rustdoc named another ecosystem; reworded (038aefac).
- Gate 137 (`/tmp/gate137-ingest-muse.log`): rc=0, validate PARTIAL — every selected node passed (209), 7 delegated-live-scope cross checks skipped as usual. (Gate 136 was stopped early on the rustdoc failure and superseded.)
- Remaining wrkviz ingest: `ingest-orc` (SQLite exception), `ingest-project`; then `summarize`, `build`, composites; then the edition switch at parity.

## Slice 6e landed: Rust `ingest-orc` (main e3235644)

- Fifth and last per-provider ingest normalizer at differential parity on the shared writer. The 7,405-line SQLite exception (`py/wrkviz/orc.py`) landed in `rs/wrkviz/src/orc.rs` (~10,300 lines) because its decoding is SQLite, not transcript records: manifest record types with versioned schema (v1/v2 source copies), ContinuationSpec/Link, SourceCopy, the rusqlite backup layer, discovery (index.db + continuation sources), task projections, logical state with BigUint append watermarks, semantic identity/validation, prefix diff + operator override, snapshot candidate prepare/advance/publish/prune, continuation-boundary resolution with evidence-based table adoption, and `load_orc_team`. Wired via `load_orc_manifest` (schemas 1-5) + `ingest_orc` in the shared writer and the `ingest-orc` CLI with verbatim help.
- Differential `py/tests/test_wrkviz_ingest_orc_rust.py` (12 cases, fixture-only SQLite): fresh + second run, missing root, malformed database, append rewrite refused then accepted with `--accept-orc-prefix-rewrite`, upstream-retired lineage, continuation lineage (plain and bounded `start_message_id`), legacy `content_blocks`/`conversation_state` session, duplicate index rows, duplicate `v2-block` turns, planted staging leftovers, window + identity overrides. All 104 wrkviz Rust differentials pass together.
- Adversarial review (one subagent, 12 findings): last-row-wins dict semantics for duplicate index/`v2-block` ids, staging token uniqueness inside one clock tick + create-new projection staging, `.db-wal`/`.db-shm` pruning, char (not byte) JSON-diff cap, live-path symlink proofs propagating, single-wrapped backup errors with bare journal refusal, sorted manifest field lists, an agent-path cycle refusal (the oracle dies with RecursionError), and decompressed-copy cleanup at ingest end. The port also caught mis-stated prefix-diff bounds (context 24, JSON paths 8, depth 6, message 4000, JSON chars 1,000,000, keys 8).
- Named divergence: SQLite online-backup bytes carry the writer library's `SQLITE_VERSION_NUMBER` header stamp (CPython's linked library vs the bundled one), so backup digests and the content-addressed paths derived from them differ per edition — the environment-defined stamp class, like zlib-generation bytes. Payload digests (append prefixes, semantic identities, task projections) and every other byte agree; snapshot databases compare by SQL dump in the differential.
- Gate 139 (`/tmp/gate139-ingest-orc.log`): 208 passed; the only failure was a strict-mypy finding in the new test helper, fixed and re-verified solo (`repository.check.python-types`, both wrkviz shards, `rust.wrkviz.test` all pass on the final commit e3235644).
- Remaining wrkviz: `ingest-project`, `summarize`, `build`, composites (refresh/export/gc/migrate-*/audit-*/responsiveness/discover-agents/`--userguide`), then the edition switch at parity, then generated schemas (single-Rust-source-of-truth collapse).

## Slice 6f landed: Rust `ingest-project` (main 12590adc)

- Composite project ingest at differential parity: strict schema-v1 manifest loader (`rs/wrkviz/src/project_config.rs`, ~1,300 lines) with the five-provider dispatch (claude-history/codex/claude/muse/orc), TEAM:SESSION rewrite selection, per-team failure collection (exit 1 with other teams still landed), and `ingest_project` on the shared writer; `extract_transcripts_archive_with_rules` added to `rs/wrkviz/src/transcript_export.rs`. CLI `ingest-project` in `rs/wrkviz/src/cli.rs` with verbatim help.
- Differential `py/tests/test_wrkviz_ingest_project_rust.py` (7 cases, fixture-only): mixed codex+orc teams, second run, team filter, one failing team among good ones, bad configs (unknown provider, malformed JSON, missing fields), accept-selection errors, and a claude-history team beside a covered claude team. All 111 wrkviz Rust differentials pass together.
- Adversarial review (one subagent, 7 findings): fixed the missing history-exclusion summary line on the project receipt path (the claude path already printed it; the new differential case proves byte parity), widened the errno→error-class map (1/13 PermissionError, 4 InterruptedError, 11 BlockingIOError, 110 TimeoutError), `required_string` now strips U+001C..U+001F, output/home expansion is fallible so `~user` or a missing HOME refuses by name (Python crashes there; named divergence), and `schema_version` parses as i128 so out-of-i64 versions refuse by value. Accepted named gaps: provider-fabricated error_type names and dropped tracebacks, config JSON/UTF-8 error wording, non-UTF8 argv/file stems.
- Gate 140 (`/tmp/gate140-project.log`): every selected node passed (209), 7 delegated-live-scope cross checks skipped as usual.
- Remaining wrkviz: `summarize`, `build`, composites (refresh/export/gc/migrate-*/audit-*/responsiveness/discover-agents/`--userguide`), then the edition switch at parity, then generated schemas (single-Rust-source-of-truth collapse).

## Slice 7 scoping: Rust `summarize` (not yet landed)

- Oracle surface: `summarize_archive`/`_summarize_archive_locked`/`plan_summarize_archive` in `py/wrkviz/pipeline.py` (tree build, plan, rollups, naming, overview), `py/wrkviz/summarize.py` (2,073 lines: jobs, input hashing, prompt build, backend output parse + repairs, cache, carried-forward), `budget.py` (611: prices, plan estimate, SpendGuard), `summary_registry.py` (601), `summary_artifacts.py` (296), `summary_catalog.py` (286), `backend_process.py` (111), `claude_backend.py` (167), plus `phases.py`/`periods.py`/`token_usage.py` pieces. CLI: `_summary_call` + `_print_summaries` with the full `--summary-*`/`--rollup-kind`/`--budget-usd`/`--price` surface.
- Backends: `heuristic` (deterministic/offline — the differential vehicle), `codex` and `claude` (subprocess wrappers; `--codex-command`/`--claude-command` exist "primarily for testing/wrappers", so fake wrapper scripts drive them in fixtures, exactly as `py/tests/test_timeline_summarize.py` does).
- Staging: (1) engine + heuristic backend + registry/artifacts/budget-plan + CLI, differential on a small ingested archive (fresh, second run, budget refusal exit 3 via `_PlanOverBudget`, rollup kinds, window bounds); (2) codex/claude subprocess backends with fake wrappers. Archive reading reuses the slices 4a-4d model code. `refresh*` composites stay for the composites slice.

## Slice 7 landed: Rust `summarize` (main 8553ce57)

- The full model-backed computation substrate at differential parity: registered summarizer contracts with their deliberate former-name prompt versions (`summary_registry`), context-coverage provenance, artifact envelopes and the logical-key catalog, token-usage receipts, spend planning (calibrate/SpendGuard/run-state), the summary cache with legacy identities and rollup own-inputs carry-forward, strict backend-output parsing with timestamp repair, and hindsight agent naming. Tree: `build_phases`/periods (DST fold-0 midnights), frozen project-overview epochs with source-set renewal, whole-tree planning, bottom-up rollups, writers, and the `summarize` subcommand with verbatim help, `--plan`, and budget refusal (exit 3).
- Backends: `heuristic` (deterministic), plus Codex/Claude subprocess backends driven by fake wrapper commands in fixtures. Backends run in process groups; SIGINT terminates the groups, records an interrupted run, and exits 130 (verified manually with a sleeping fake).
- Differential `py/tests/test_wrkviz_summarize_rust.py` (10 cases): heuristic fresh/second run, multi-agent (root+child) team, `--plan`, budget refusal, fake Codex, fake Claude, failing backend exit 2, daily-only rollups, missing team, and a stateful-wrapper case proving split-batch retry fatality. 120 wrkviz Rust differentials pass together; clippy and rustfmt clean.
- Adversarial review (12 findings): fixed the split-batch extra retry, bounded the worker pool to `--summary-workers` with queued-batch cancellation, added the SIGINT/process-group path, canonical prompt/schema/audit byte shapes, shared Claude transport for naming, DST fold-0 midnights, naming shorten/titlecase parity, glossary-job validation, and `--p` ambiguity. Accepted named gaps: CPython-vs-serde JSON error wording and Python traceback text (campaign-wide classes).
- Gate 141 (`/tmp/gate141-summarize.log`): all wrkviz steps passed; the only failures were rustfmt (fixed in 8553ce57) and two 120s host-load timeouts, all three re-verified solo on the final commit (`/tmp/gate141-solo.log`, rc=0).
- Remaining wrkviz: `build`, composites (refresh/export/gc/migrate-*/audit-*/responsiveness/discover-agents/`--userguide`), then the edition switch at parity, then generated schemas (single-Rust-source-of-truth collapse).

## Slice 8 scoping: Rust `build` (not yet landed)

- Oracle surface: `_build_archive_locked`/`build_archive` in `py/wrkviz/pipeline.py` (~5410-5660: cached phase/name/overview/rollup loading with catalog selection and stale/partial suppression, `_ensure_archive`, precompress) and `py/wrkviz/render.py` (1,767 lines: phase/rollup/agent Markdown, timeline JSON edges and events, standalone server source, archive Makefile/README). Loaders `_load_phase_summaries`, `_load_agent_names`, `_load_project_overview`, `_select_catalog_summary`, `_summary_projection_missing`, `_load_rollup_projection` sit between the slice-7 writers and the renderer.
- Reuse: slice 7 already ported phases/periods/provenance/catalog and the summary JSON shapes the renderer consumes; the site assets (JS/CSS/HTML) carry over unchanged. Zero tokens: build never calls a backend, so the differential is a pure fixture comparison (ingest + summarize-heuristic, then build in both editions and compare the generated trees).
- Staging: (1) loaders + build orchestration; (2) render.py port (Markdown/JSON/Makefile/README/standalone); (3) CLI `build` with verbatim help. Composites (`refresh*` = ingest+summarize+build chains) stay for the composites slice.

## Slice 8 landed: `build` in Rust (2026-10-31)

`wrkviz build` is ported at differential parity: cache loaders
(phase, agent-name, rollup, project-overview, artifact catalog,
site identity, pull-metadata cache), `render_archive` (Markdown
phases/agents/rollups/glossary, per-phase detail documents,
schema-3 timeline/search/bloom/link shards, stale-presentation
retirement, retired `query.py` pruning), `build_archive` under the
archive writer locks, and the `build` CLI including the run-record
`build` payload. Landings: `f9caee5f` (stage B1 helpers and the
schema-3 writer), `c2541e22` (loaders, render, CLI), `8455f239`
(differential and registration), `9dbfa84e` (rustfmt), `32e695d5`
(adversarial-review fixes) on main. The review's two highs are
closed: generated paths screen relatives and refuse parent
symlinks like the oracle, and legacy project-overview validation
(epoch shape, transcript digest, summary contract) is complete.
`GlossaryTerm` is consolidated to one Rust type
(`build_assets.rs`), per the single-source-of-truth rule.

Gate 143 (`/tmp/gate143-build.log`) on `32e695d5`: every selected
step passed in some run on the final commit; the only failure,
`rust.agentctl.test`, passed solo
(`/tmp/gate143-solo.log`). New differential:
`py/tests/test_wrkviz_build_rust.py` (9 cases: no summaries,
summarize-then-build plus idempotent rebuild, multi-agent edges,
`--phase-minutes`, `--help`, missing team, tampered legacy
overview, invalid pull cache, parent-symlink refusal). Trees
compare gzip members by decompressed content and scrub only
zlib-generation fields (compressed sizes/digests and shard byte
totals).

Coordinator's full gate on slice 7's `e8cd2b64`
(`/tmp/coord-gate-slice7.log`): every selected node passed
(PARTIAL only from delegated-live-scope skips), so slice 7 is
verified on the commit that landed.

Remaining wrkviz: composites (refresh/export/gc/migrate/audit/
responsiveness/discover-agents/userguide), the edition switch at
parity, and the generated-schema collapse.

## Slice 9 scoping: composites, in porting order

Remaining unported commands (help sizes in lines: export 26,
gc 20, migrate-build-state 27, migrate-snapshots 28,
github-metadata 14, audit-glossary 14, audit-losslessness 19,
refresh 124, refresh-claude 121, refresh-orc 139,
responsiveness 50, discover-agents 50, `--userguide`):

1. `export` — CLI-side window parsing over the landed
   `build_archive` output/display-window support; multi-team
   alignment is the open question at slice start.
2. `gc` — manifest-authoritative file classification
   (`py/wrkviz/gc.py`), report first, `--delete` to trash.
3. `migrate-build-state` / `migrate-snapshots` — filesystem
   moves with existing Rust store helpers nearby.
4. `github-metadata` — cache load/validate already ported for
   build; enrichment fetch is the remaining piece.
5. `refresh`, `refresh-claude`, `refresh-orc` — orchestration
   over landed ingest/summarize/build.
6. `audit-glossary`, `audit-losslessness` — report generators.
7. `responsiveness`, `discover-agents`, `--userguide`.

Each lands with its own fixture differential, gate, and note,
same rules as slices 1-8.
