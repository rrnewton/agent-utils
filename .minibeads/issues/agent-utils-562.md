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

[muse watermelon] Slice 11 landed (0ae36782): `parse-journal` also covers
recover-absent-validate-rows (journal identity, per-row record digest,
and the exact derived ARCHIVED entry per row). The ownerless-agent,
ownerless-clone, cache-relocate, and absent-agent-row kinds remain.

[muse watermelon] Slice 12 landed (aa9cd253): `parse-journal` also covers
ownerless-agent-remove, including its directory-identity check (device,
inode, mount id) on a present target and repository resolution. The
ownerless-clone, cache-relocate, and absent-agent-row kinds remain.

[muse watermelon] Slice 13 landed (a9ac301c): `parse-journal` also covers
ownerless-clone-remove (clone authorization, target/fenced agreement,
directory identity). Remaining: ownerless-agent-cache-relocate and
recover-absent-agent-row.

[muse watermelon] Slice 14 landed (2dc67466): `parse-journal` also covers
ownerless-agent-cache-relocate (host identity, canonical cache paths,
fence, directory identity). Remaining: recover-absent-agent-row.

[muse watermelon] Slice 15 landed (0eaa9f58): `parse-journal` also covers
recover-absent-agent-row (row identity and record digest, branch
witnesses, remote containment, submodule salvage receipts, planned
rescue receipts, and the exact derived ARCHIVED entry under both rescue
roots). That completes every recovery-journal kind.

[muse watermelon] Slice 16 landed (b2ab2951): validation/editions.json updated for
complete journal coverage. After this, the remaining work toward a Rust
wrkslots is composed read commands (list/status-style summaries over the
validated formats), then writers.

[muse watermelon] Slice 17 landed (1a1e6271): `wrkslotsd parse-states`
composes the ACTIVE/ARCHIVED readers with `_ensure_state_shard`'s
partial-shard pairing rule (snapshot mode; EVENTS derivation stays with
`rebuild`). Gate 28 failures were load flakes only: cross-skip-accounting
(71 passed alone), vibe-talk page-1 shift-o-latency (passes alone), and
wrkslots shard 1 (468 passed alone) each passed in a solo rerun, so every
selected step passed in some run. Next: further composed readers or the
first writer slice.

[muse watermelon] Slice 18 landed (7fea9b49): `wrkslotsd parse-global`
composes the per-machine shard readers as one registry (machine-shard
and event-directory discovery, shard-set disagreement without history,
cross-machine slot/agent uniqueness, active-versus-archived overlap) in
snapshot/reference mode; EVENTS-derived machines refuse toward `rebuild`
and the Python lock-based overlap re-read is not reproduced. Adversarial
review found the event-log entry classification, error precedence, and
first-conflict ordering divergences; all fixed with differential cases
(14 passed). Gate 29 failures were load flakes only (cross-skip 71
passed alone; wrkslots shard 0 476 passed alone). Note: the landed
`parse-states` (slice 17) keeps the simpler any-entry event check and
should adopt the same classification. Next: slice 19 parse-holds.

[muse watermelon] Slice 19 landed (44764349): `wrkslotsd parse-holds`
composes the active-shard and hold readers per `_load_record_holds`
(one entry per active record across all machines, per-slot errors
captured) in snapshot mode; EVENTS-derived machines refuse toward
`rebuild`, an invalid event log becomes each record's captured error.
Adversarial review found the corrupt-chain classification, error
precedence, and partial-name edges; all fixed with differential cases
(11 passed in the new suite; library-generated error suffixes remain a
known low divergence). Gate 30 failures were load flakes only
(cross-skip 71 passed alone; wrkslots shard 0 481 passed alone).
Next: slice 20 gives `parse-states` the same event-log classification.

[muse watermelon] Slice 20 landed (09958917): `wrkslotsd parse-states`
adopts the shared event-log classification (complete event file
derives toward `rebuild` after chain validation, unexpected entry
refuses with the event loader's message, an in-progress write of the
first event file refuses with the bootstrap's partial-update message
after shard validation). Adversarial review found the precedence,
symlinked-log, and corrupt-chain divergences; all fixed with
differential cases. Gate 31 failed only on validation-graph (timeout
under load), passed alone (42 passed).

[muse watermelon] Slice 21 landed (2a033777): `wrkslotsd parse-journals`
reproduces `_outstanding_journals` (control-dir journal files plus
operations still pending in each machine's validated event history).
Adversarial review found unvalidated EVENTS machine names and the
bare `ACTIVE.journal` glob edge; both fixed with differential cases
(8 passed). Gate 32 failures were load flakes only (cross-skip and
validation-graph 113 passed together alone; wrkslots shards 488 + 488
passed alone). Upstream vibe-talk commits shared no files; rebased and
pushed without a second gate.

[muse watermelon] Slice 22 landed (5a4fc874): `wrkslotsd parse-status`
reproduces the per-record status object over snapshot states (owner
process diagnosis from this host's /proc, boot and machine identity,
heartbeat age/expiry, hold outcome). Registry storage inconsistencies
and event-derived owner releases stay with the Python `status`.
Adversarial review found the fractional-second heartbeat expiry and
the hold-error refusal; both fixed with differential cases (status
suite 3 passed, 13 with holds). Gate 33 failures were load flakes only
(cross-skip 71 passed alone; wrkslots shard 0 490 passed alone).
Upstream vibe-talk commits shared no files; rebased and pushed
without a second gate.

[muse watermelon] Slice 23 landed (c91fc914): `wrkslotsd parse-doctor`
composes tolerant status records with the snapshot-scope doctor
findings (invalid hold, dead owner, unfinished journal, partial
atomic update, cap breach). Registry storage, slot-image, and
owner-release findings stay with the Python `doctor`. Adversarial
review found the machine-scoping divergence (default local machine,
`--all-machines` for every machine); fixed with differential cases
(3 passed). Gate 34 failures were load flakes only (cross-skip and
validation-graph 113 passed together alone; wrkslots shards 491 + 491
passed alone). Upstream commits shared no files; rebased and pushed
without a second gate.
