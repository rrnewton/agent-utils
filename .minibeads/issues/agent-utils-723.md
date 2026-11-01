---
title: 'rust-primary-schema-collapse: generated schemas from single Rust types'
status: open
priority: 2
issue_type: task
created_at: 2026-11-01T10:53:26.239531791+00:00
updated_at: 2026-11-01T10:53:26.239531791+00:00
---

[muse watermelon] Plan: ai_docs/transient/2026-11-01-schema-collapse/PLAN.md
One Rust type per format, versioned generated schema checked in, Python
mirrors validate against it. First slice: agent-queue. No version bumps,
no default flips, no publishing, no parallel-experiment-runner.

Slice 1 (agent-queue) implemented: QueueItem, Tombstone and
QuarantineSidecar types in rs/agent-queue/src/schema.rs (schemars 1.2.3,
pinned in Cargo.lock, default-features off); generated schemas in
validation/schemas/; regenerate-and-diff checker
scripts/check_agent_queue_schemas.py wired as
repository.hygiene.agent-queue-schemas{,-self-test} (fail closed);
Python validates in tests only via a minimal in-repo validator
(jsonschema is not vendored). Target binding stays with its agentctl
slice. No format version bump: schemas describe the current format,
including open sender fields (additionalProperties: true).

Slice 2 (wrkslots state and handoffs): ActiveState/ActiveRecord,
ArchivedState/ArchiveRecord, HandoffSidecar (schema 2) and
HandoffSidecarLegacy (schema 1) typed in rs/wrkslotsd/src/formats.rs;
generated schemas validation/schemas/wrkslots-*.schema.json; checker
scripts/check_wrkslots_schemas.py as repository.hygiene.wrkslots-schemas
{,-self-test}, fail closed; Python test-only validation in
py/tests/test_wrkslots_state_schema.py. Field-set comparison: the
written field names and types agree in both editions (the differential
corpus holds them at parity). The adversarial review then found, and
this slice now reports rather than normalizes: (a) slot_type, layout,
task_scope and salvage are absent-or-valued in both editions, never
null -- the schemas now refuse explicit null for them; (b)
coordinator_lease is non-null on every valid active record and is
typed so; (c) SourceIdentity loaders in both editions accept extra
keys while writers emit exactly three -- the schema allows extra
keys there and says so; (d) integer width: Python ints are unbounded,
schema.rs positive() accepts beyond i64::MAX, the state loaders'
as_int refuse it -- pre-existing, reported here, not resolved by this
slice; (e) TaskScope unit format controls: Python and
evidence::ScopeIdentity refuse Unicode Cf, active_state's reader
accepts them -- pre-existing Rust-internal disagreement, reported,
not resolved. Digest/identity binding rules stay in the validators.
No version bump; config/hold/journal formats are later slices.

Slice 3 (wrkviz archive records, schema-3): TimelineV3Bootstrap with
Codec, Streams/StreamSection and ShardCatalogEntry typed in
rs/wrkviz/src/formats.rs; generated schema
validation/schemas/wrkviz-timeline-v3-bootstrap.schema.json; checker
scripts/check_wrkviz_schemas.py as repository.hygiene.wrkviz-schemas
{,-self-test}, fail closed; Python test-only validation in
py/tests/test_wrkviz_archive_schema.py. Snapshot pointer (schema 1)
and streamed record lines are separate formats, later slices.
Per coordinator: after slice 3 lands, pause wrkviz work for the
WRKVIZ_ENGINE=rust trial.

Review findings for slice 3, fixed: teams must name at least one
slug; search stream sections refuse explicit null (all-or-nothing
stays a reader rule); line_ranges pairs are unsigned; the Python
validator now enforces maxLength, item counts and map-value schemas.
Reported, not normalized: readers accept documents the writer schema
refuses (unknown keys, omitted writer-only fields) -- the schema is
the writer contract; integers beyond i64 are accepted by the Python
reader and refused by the Rust reader; NaN/Infinity literals are
accepted by the Python JSON parser and refused by the Rust bootstrap
reader; the Python writer passes target_chunk_bytes through
unchecked where the Rust writer's u64 prevents a negative value.
