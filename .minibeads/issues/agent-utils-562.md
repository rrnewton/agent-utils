---
title: 'rust-primary-wrkslots: port wrkslots to Rust behind the same CLI'
status: open
priority: 2
issue_type: task
created_at: 2026-10-27T00:49:51.191658938+00:00
updated_at: 2026-10-27T00:49:51.191658938+00:00
---

[muse watermelon] rust-primary campaign milestone 4: port wrkslots to Rust
incrementally behind the same CLI, growing rs/wrkslotsd subcommand by
subcommand, with the existing Python tests driving a differential harness.
Switch the primary edition in validation/editions.json only at parity; the
Python edition then becomes a paused mirror unless parity is cheap.

First slices should be pure readers with exact output parity (status/doctor
over a fixture checkout) before any mutating command moves.

[muse watermelon] Groundwork notes: Python wrkslots is the slot-manager CLI
(~62k lines of cli.py alone, ~40 subcommands); rs/wrkslotsd today is a
read-only observer over EVENTS logs (rebuild/status/explain/plan), a
different surface. First slice plan: build a fixture corpus from the Python
tests (init/create/status --format json over a temp project) and land a
wrkslotsd reader that reproduces `status --format json` for that corpus
byte-for-byte before any mutating command moves. `chat launch` stays held
until the delivery-latency lane lands; migrate/repair fold in here via the
herdr-subagents runtime when this port reaches turn-runner workers.

[muse watermelon] Slice 1 landed: Rust document loader (wrkslotsd parse-config) at differential parity with the Python loader (17-case corpus). Next slice: config validation (the load_config semantics: schema, inheritance, normalization) in wrkslotsd behind a read-only command, same differential pattern.

[muse watermelon] Slice 2 scoping: `_load_config` (py/wrkslots/cli.py)
discovers the root, exact-keys the document, then validates schema,
machine, worktrees/control directories, remote, landed ref, liveness
commands, caps, globs, hooks, disk thresholds, and representation/image/
sandbox settings. Rust slice 2 = `wrkslotsd validate-config --root DIR`
reproducing the resulting Config as canonical JSON for fixture roots built
by the Python tests, error-for-error on the refusal cases. No mutation.
