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

[muse watermelon] Slice 2 landed (358be730): `wrkslotsd validate-config`
reproduces `_load_config` validation as canonical JSON, error-for-error on
refusals, over a Python-built fixture corpus (test_config_validate_rust.py).
Next slice: a pure state reader (status/list JSON over fixture slot state).

[muse watermelon] Slice 3 landed (3ec1769f): `wrkslotsd parse-state` reproduces
the ACTIVE.<machine>.json shard loader in reference mode
(`_load_active_snapshot(..., require_repository=False)` + `_active_to_obj`):
record identity/handoff/checkout/task-scope/import sub-formats, duplicate
and machine-shard checks, and stored-path authority against the validated
configuration. No live-process, Git, or checkout-existence inspection.

[muse watermelon] Slice 4 landed (8d6d86c1): `wrkslotsd parse-archive` reproduces
`_archive_from_obj` / `_load_archive_snapshot`: retirement evidence,
archive-id derivation, historical-import consistency, and archived
checkout path/authority, emitting records as stored.

[muse watermelon] Slice 5 landed (109abe9a): `wrkslotsd parse-handoff` reproduces
`_load_handoff_sidecar` bound to its active record (bounded identity-bound
file read, strict duplicate-refusing JSON, schema 1/2 provenance rules,
content digest binding) and `_handoff_artifact_to_obj` output.

[muse watermelon] Slice 6 landed (9d3666e1): `wrkslotsd parse-hold` reproduces
`_load_hold_snapshot` for the HOLD.<machine>.<slot>.json record
(absent = null; exact keys, schema, filename identity, timestamp, reason).

[muse watermelon] Slice 7 landed (cdbcb513): `wrkslotsd parse-journal` reproduces
`_validate_journal_shape` for create, import-existing, and finish journals
(filename agreement, planned/created shapes, embedded records, finish
phase/archive-id and optional evidence bundles). Other kinds still refuse.

[muse watermelon] Slice 8 landed (e47d3cfd): `parse-journal` also covers the
legacy-validate-remove journal kind (exact fields, actor identity,
coordinator authorization flag).

[muse watermelon] Slice 9 landed (648ee8a6): validation/editions.json now records the
slice 1-8 format readers (the wrkslotsd tool note and the wrkslots format
inventory entry name the Rust modules, the differential corpus, and the
remaining ownerless/absent journal kinds).

[muse watermelon] Slice 10 landed (df82d2a0): `parse-journal` also covers
ownerless-validate-remove, including the validation-recovery authorization
and, when present, the exclusion path validation (recovery target, fenced
path, guard identity). The ownerless-agent/clone/cache and absent-row
journal kinds remain unsupported.

[muse watermelon] Remaining journal kinds are the ownerless-agent/clone/cache and absent-row families. Next: scope the ownerless-agent journal inputs.
