---
title: 'wrkslots-rust-transfer-parity: close the Rust transfer-owner lock-scope gaps before cutover'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- rust-cutover
created_at: 2026-11-20T14:43:46-05:00
updated_at: 2026-11-20T14:43:46-05:00
---

# Description

[watermelon-full-preview] Filed from the #1975 restore-boot-recovery Phase 1 cold review (2026-11-20). Phase 1 gave `transfer-owner` the lock-light prevalidated treatment in both editions. The Python edition (authoritative, shipped) compares the registry identity under the locks, defers to the original fully-locked body on an outstanding journal, and falls back to that body when the prevalidated rounds exhaust (`_RegistryBusy`). The Rust edition (`rs/wrkslotsd`, writers fixture-registries-only until cutover) has three gaps the review named: (1) its prepare can return Already without the outstanding-journal check, diverging from Python's refusal; (2) it has no registry-identity compare and Phase 1 dropped its post-write `validate_global`, so a `--to-agent X` transfer racing an adopt/create could give X two slots between the lock-free judge and the locked write; (3) it loads the full event chain under the locks on every commit, the cost Python's identity compare exists to avoid. None of these can bite before cutover (fixture registries only), but cutover on this shape would import a liveness race into the reboot re-attach storm Phase 1 exists to fix.

# Design

Bring the Rust transfer-owner to the Python contract before the wrkslots Rust cutover enables its writers on real registries:

(1) Journal parity: the Rust prepare must check for an outstanding journal before any Already/short-circuit return, and defer to a fully-locked body with the same refusals as Python's `locked_body` (journal classification and dead-actor repair stay under the lock).
(2) Race closure: add the registry-identity compare (the `_registry_identity` analogue over ACTIVE/ARCHIVED/journals/EVENTS tips and image records) so the locked commit re-judges against the state prepare validated; OR restore a post-write `validate_global` plus an only-this-slot-changed assertion, matching whichever subset the Rust heartbeat settles on — the invariant to pin is that no agent can end up owning two slots via a transfer racing an adopt/create.
(3) Lock scope: with the identity compare in place, load the event chain in prepare (lock-free) and only verify/append after the verified tip under the locks, as Python does; add the `_RegistryBusy` fallback to the locked body so a transfer storm serializes instead of refusing.
(4) Extend the differential lock-scope tests (the `test_transfer_lock_scope.py` ledger treatment) to drive the Rust binary through the same busy, journal, and rename-race scenarios.

# Acceptance Criteria

- The Rust transfer-owner refuses with an outstanding journal exactly where Python does (differential test, journal bytes unchanged), including the already-owned case.
- A differential race test (transfer `--to-agent X` interleaved with adopt/create for X between prepare and commit) never leaves X owning two slots in either edition.
- The Rust lock-scope ledger shows zero fleet work (global validation, storage scan, event-chain load) under the mutation locks at many rows, and a forced-busy storm of transfers all succeed through the locked fallback with normal outputs and events.
- Outputs and event payloads stay byte-identical between editions across the owner-transfer differential suite; no persisted fields change.
- The fix lands before, or as a gating item of, the change that enables Rust wrkslots writers on real registries.

# Notes

Related: #1975 restore-boot-recovery (Phase 1 introduced the lock-light transfer in both editions; the Python side is complete there). Not a Phase 1 blocker: Rust writers are fixture-registries-only until cutover. Persisted fields: none — parity of behaviour, not of storage.

Update (from #2000 wrkslots-lock-order): [watermelon-full-preview] The Rust mutation-lock configuration gate in `hold_write::acquire_mutation_locks_inner` now compares the configuration file's SHA-256 (mirroring the Python registry identity's `config_sha256`) plus every field the Rust `Cfg` carries, instead of only six fields. What remains weaker than Python here, and stays inside this issue's existing scope, is the registry-identity compare itself: the Rust edition still has no `_registry_identity` analogue under its mutation locks (item (2) above), so a Rust writer does not yet re-judge against the exact registry identity its prepare validated.
