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
