---
title: 'wrkslots-provenance-only-slot-offline-probe: create must not refuse an offline slot at its remote probe'
status: closed
priority: 2
issue_type: feature
labels:
- wrkslots
- offline
created_at: 2026-11-19T19:40:00-05:00
updated_at: 2026-11-20T06:30:00-05:00
closed_at: 2026-11-20T06:30:00-05:00
---

# Description

[watermelon-full-preview] A consumer reports: "wrkslots refused a provenance-only slot today because its probe can't reach our GHE host." The create-time probe is `fetch_remote` (`git ls-remote` + `git fetch`) in `_create_plan`, run for every non-validate checkout. A slot that does not need the remote (provenance-only, read-only, or offline) must not be refused by it.

# Design

- `create --offline-reason TEXT` (non-empty): a declared offline/provenance-only slot. `_create_plan` skips `fetch_remote`; the start ref must still resolve locally (`verify_ref`) and the remote authority is still read locally and recorded as today. Normal slots without the flag always probe and still refuse on an unreachable remote: never silent.
- The reason is recorded only in the `slot-created` event's evidence (`evidence.offline_reason`), which old readers treat as opaque for `slot-created`. No new record field, journal field, event kind, or file, so a consumer pinned at writer_ceiling 2 and the pinned readers 096a8d74 / 74549a37 replay, status, doctor, audit, finish and remove such a slot unchanged.
- Cannot combine with `--eden`. Rust `create` / `create-text` parity with the same flag, skip and evidence.
- Tests with an unreachable fake remote drive the exact defect.

# Acceptance Criteria

- A normal create against an unreachable fake remote refuses; the same create with `--offline-reason` succeeds and its checkout HEAD matches the local start ref.
- The slot's active record and create journal carry no offline field; the `slot-created` event evidence carries the reason.
- An offline create whose start ref does not resolve locally refuses; an empty reason refuses.
- Pinned readers 096a8d74 and 74549a37 run status, doctor, audit, finish and remove on a scratch offline slot.

# Notes

Coordb approved the evidence-only recording (2026-11-19): gating at schema level 3 would make the flag useless to the writer_ceiling 2 consumer, and an ungated record field would make their old reader refuse remove/finish.

## Resolution

[watermelon-full-preview] Landed on main in fbaa6ef6 (landed as dc2c5fdb): create --offline-reason TEXT skips the create-time remote probe; reason recorded only in slot-created event evidence. Old readers 096a8d74/74549a37 status/doctor/audit/finish/remove proven on a scratch offline slot. Focused suites and gate passed (gate reds only waived environment families).
