---
title: 'wrkslots-prepared-remove-tolerates-foreign-events: prepared removes defer on any foreign event'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- lock-scope
created_at: 2026-11-18T12:30:00+00:00
updated_at: 2026-11-18T12:30:00+00:00
---

# Description

[watermelon-full-preview] Consumer measurement on the #1807 build (f1697dc3), 2026-11-18: no agent remove has succeeded on the consumer host since that build deployed. A prepared (retire-pending / prepared agent) removal validates lock-free for minutes, then `_assert_prepared_removal_current` compares the whole registry identity — every file under `EVENTS.<machine>/` included — and defers the slot when ANY other slot's event landed meanwhile. At the consumer's event rate (one per 1-3 minutes) a multi-minute validation can never commit; disk reclamation is stalled.

The identity refusal runs before the #1807 branch that advances the removal past another slot's event when this slot's own record is unchanged, so that branch is unreachable for ordinary event-log growth.

# Design

Reorder `_assert_prepared_removal_current`: judge the current log-derived state first. Three gates made the #1807 advance unreachable, and all three now judge only this slot: (1) a registry identity change is tolerated only when every changed entry is an ACTIVE/ARCHIVED shard, an EVENTS directory entry, or an appended (never modified or removed) EVENTS file — the configuration, journals, seals, quarantine and slot-image records must be untouched; (2) the journal fingerprint, which also covers every event file, is compared on its journal entries only; (3) the lock-scope fingerprint is compared on this slot's generation, record digest, checkout heads and the configuration, not its whole-shard hash. The target slot's current record must still equal the prepared record, or the removal defers by name. Foreign events fold into the current state the commit advances (the writer is already repositioned at the chain tip under the lock, #1807). The Rust edition's prepared remove drops its event-chain-length refusal the same way: the re-read target record decides.

Regression test from the measured scenario: foreign events (another slot's create and heartbeat) landing during the lock-free phase of a retire-pending candidate; the remove completes, and a change to the target slot itself still defers.

# Acceptance Criteria

- A prepared removal completes when other slots' events land during its lock-free validation (deterministic test, both editions where the surface exists).
- A change to the target slot's record, its checkouts, the configuration, or any journal still defers/refuses exactly as before.
- Existing retire/remove/lock-scope suites pass.

# Notes

Found on top of #1807; the fix builds on #1808's tolerated-change work (stage 1). The consumer pins this fix as soon as it lands.
