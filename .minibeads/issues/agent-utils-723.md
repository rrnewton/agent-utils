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
