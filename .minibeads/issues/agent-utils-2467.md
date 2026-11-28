---
title: 'local-ci-hub-s1-usable-binary: tracked Rust entry point, TOML config, runnable quick-start'
status: closed
priority: 1
issue_type: task
assignee: mu-rustport
labels:
- local-ci-hub
- rust-primary
created_at: 2026-11-28T09:05:00-05:00
updated_at: 2026-11-28T11:56:00-05:00
closed_at: 2026-11-28T11:56:00-05:00
---

# Description

The Rust crate in `rs/local-ci-hub` is a complete read-only ledger core — typed events, one fold, one canonical digest, the qualifying predicate's core, a versioned SQLite index, the fan-out `Store` contract, and the read verbs (`receipt show`, `ledger rows|digest-rows|verdicts`) — with no tracked entry point. `validation/editions.json` records the Rust mirror as `paused`, so the shared launcher refuses every Rust-edition request and the Python edition is the only runnable path. The crate also reads only its JSON deployment config, while both the Python edition and deployed configurations use the TOML `local-ci-hub.toml` shape.

This is slice S1 of the #1365 parity plan (owner directive 2026-11-28: port the consumer's ci-hub to the Rust successor, feature complete, before adoption). It makes the Rust edition runnable and usable on a clean machine without changing any verdict semantics: a tracked entry point, TOML configuration, an example config, a quick-start a stranger can actually run, and CLI help that tells the truth.

The Python CLI's four known help defects must not be carried over: no per-command help text anywhere; `--json` documented as accepted everywhere but only parsed before the subcommand; `run start --wait` accepted but ignored; `receipt verify` advertised but silently aliased to `show`.

# Design

- Tracked entry point `rs/bin/local-ci-hub` following the suite's existing Rust launcher convention, and the editions record for `local-ci-hub` flipped so the launcher will run the Rust edition on opt-in (Python stays primary until the later flip slice).
- TOML config reader for the established `local-ci-hub.toml` shape (project, validate, ledger, remote, receipt, health, land sections), strict about unknown keys, with the existing JSON deployment data remaining a deployment input, not the user config. A shipped example config that the tests load verbatim.
- CLI truthfulness: help text on every verb and flag; `receipt verify` as a real verb distinct from `show` (exit 0 qualified, 1 not-qualified, 2 no-result); `--json` accepted in the documented position; no accepted-but-ignored flags (the run-engine verbs arrive in a later slice and are not stubbed here).
- Quick-start in the crate README and the user docs: clone, build, copy the example config into a toy git repository, run the read-side loop against a fixture ledger. No internal host, path, id or codename in any file; consumer-specific values appear only as configuration.
- Focused tests: entry-point end-to-end tests on the shipped binary, TOML/flag precedence and refusal tests, and a clean-room quick-start test that runs the documented steps. New test files classified in `validation/components.json` and named by their harness.

# Acceptance Criteria

- `rs/bin/local-ci-hub --help`, `receipt show` and `ledger rows` run through the tracked launcher from a fresh checkout; `scripts/check_editions.py` passes.
- A TOML config in the deployed shape loads; unknown keys refuse with a named error; flag-over-config precedence matches the JSON deployment reader's documented order.
- `receipt verify` exits 0/1/2 for qualified/not-qualified/no-result and is not an alias of `show`; every verb prints real help; `--json` parses where documented.
- The example config is loaded verbatim by a test, and the documented quick-start steps pass in a throwaway git repository (fixture ledger in, verdict out).
- `cargo fmt --check`, the crate's focused tests, and the gate's selected checks pass; changes touch only local-ci-hub surfaces plus editions/docs metadata, with no verdict-semantics change (digest and predicate bytes unchanged on existing fixtures).

# Notes

Slice S1 of the #1365 campaign; number pre-assigned by the coordinator (2026-11-28). Later slices: predicate completion, writers/connectors, run engine, metrics (both time-green forms), history queries, shadow/flip.

## Resolution

Landed on main 2026-11-28 as d1e12204 (entry point, TOML config, quick-start), 9b4a2bf0 (launcher registration in setup and the validation manifest, counted-repos allow-list completed) and 133473e0 (Iterator::find in discovery), gated as ab3a0ae8: full gate 363 steps with two vibe-talk browser steps red, both passing a solo exclusive rerun on the identical tree (rerun_passed) and waived by the liaison as load flakes in untouched code; every other step green, incomplete_steps empty.

- `rs/bin/local-ci-hub` is tracked through the shared cargo-runner; the suite dispatcher reaches the Rust edition only via `LOCAL_CI_HUB_ENGINE=rust` (Python stays primary; generic Rust requests still refuse on the paused mirror).
- TOML project configuration in the Python shape with the same discovery order and defaults; unknown keys and a delegated `digest_command` refuse by name; `--config` takes a `.toml` project config or the existing `.json` deployment config by extension.
- `receipt verify` is a real verb (0/1/2), `quickstart` prints the five-step loop, the example config loads verbatim in tests, and `--help` describes only verbs that exist.
