---
title: 'wrkslots-ignored-evidence-unbounded-events: bound salvage and finish ignored-file evidence so one removal cannot write 45 MB events'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T09:30:00-05:00
updated_at: 2026-11-18T10:45:00-05:00
---

# Description

[watermelon-full-preview] One consumer `remove --discard-ignored --archive-private-history --no-lock-budget` (rc 0) wrote seven events of about 45 MB each: one `active-state-recorded` event (seq 86499), five `operation-progress-recorded` events (seq 86501-86505) and one `archive-state-recorded` event (seq 86506). Every event repeats the same 151,006-entry `payload.evidence.salvage[0].ignored_files` list (equivalently under `payload.journal.salvage` in progress events and `payload.record.salvage` in the archive event), each entry `{path, bytes, sha256, disposition: discarded}`. The `wrkslotsd` observer rebuild then fails with "event exceeds the 16777216-byte replay limit" and the read-only observer index is down. The listed files (about 5.5 GB total) are mostly hand-archived project files under a gitignored `ignored/` directory plus a `shim/` tree; they are not regenerable build caches, so widening the cache definition is out of scope.

Origin: #1302 wrkslots-ignored-evidence-salvage (612273f9, remove salvage lists every ignored file with sha256) and #1686 wrkslots-finish-discard-ignored (c558b3e5, finish records each ignored file in the handoff and remove enforces that exact record). Both are unbounded, and every progress or refusal event that carries the same salvage evidence repeats the whole list.

Urgency check (before any fix): no command the consumer runs refuses. The Python replay (`_read_json` in `py/wrkslots/cli.py`, used by remove, recover, status, audit and doctor) has no per-event byte cap and folds the live log successfully (86,596 events in about 21 s); the Rust `load_event_chain` in `rs/wrkslotsd/src/global_state.rs` (used by the journals, recover, audit and doctor writers) is likewise uncapped. Only the observer replay in `rs/wrkslotsd/src/replay.rs` (`MAX_EVENT_BYTES`, 16 MiB) refuses, so cleanup continues but the observer index stays down until readers tolerate the existing history.

# Design

- Bounded evidence everywhere: events, the handoff and the active record carry only a summary (count, total bytes, disposition counts, the sha256 of a canonical sorted per-file manifest, the manifest file path and its sha256, plus at most 64 inline entries for readability). The full per-file manifest goes in a separate receipt file beside the salvage receipt (same directory rules, owner-only), written and fsynced before the event that references it. Remove's "enforce the exact record" check (#1686) verifies against the manifest file through its sha256. The rule covers every event kind that carries salvage or ignored evidence, including progress and refusal events: grep every writer of that evidence.
- Both editions together (Python `py/wrkslots/cli.py` and Rust `rs/wrkslotsd/src/remove_write.rs` / `rs/wrkslotsd/src/finish_write.rs`), with differential parity.
- Readers survive existing oversized history (immutable and hash-chained, so no rewrite): find every event reader (the `wrkslotsd` observer rebuild, Python replay/audit/doctor, and the Rust journals and recover writers) and ensure none wedges on an event larger than 16 MiB. For read-only replay, either raise the per-event limit for `active-state-recorded` events to a documented bound (for example 128 MiB) or stream-parse them. Keep a cap, never unlimited.
- No cache-definition change: the sampled top-level directories are reported to the coordinator and the files are project data, not caches.

# Acceptance Criteria

- A removal or finish with more than 64 ignored files writes events, a handoff and an active record whose ignored evidence is summary-only (count, total bytes, disposition counts, manifest sha256, manifest path and sha256, at most 64 inline entries); the full manifest receipt exists beside the salvage receipt, owner-only, before any referencing event.
- Remove still refuses when the live ignored inventory differs from the recorded manifest, verified through the manifest sha256.
- The observer rebuild and every other reader replay a log containing a synthetic 20 MiB event without refusing, under a documented finite cap.
- Existing differential tests for remove and finish stay green; new coverage exercises the summary shape, the manifest verification and the oversized-event replay.

# Notes

Replay cost: replaying the affected log takes about 21 s per command (86,596 events, seven of them 45 MB), so each command pays the full parse even after the fix. Whether old events could later be summarised or compacted is a separate question (history is immutable and hash-chained, so any compaction needs its own design); it is noted here for later and explicitly not done in this issue.

Overlap: the locked-registry (#1766), census-exit (#1769), dead-bind (#1771) and hook-cwd (#1768) lanes also touch `py/wrkslots/cli.py`; this change stays inside the salvage/finish-evidence and event-reader code and rebases right before the gate.

## Resolution

Landed on main as e6ee5a82. Events, the handoff and the active record now carry only the level-3 `ignored_summary` (count, total bytes, disposition counts, canonical manifest sha256, manifest path/bytes/sha256, at most 64 inline entries); the full per-file manifest is a receipt beside the salvage receipt, written and read back before any referencing event, and remove enforces the exact finish record through the manifest sha256 in both editions. Emission is gated on the #1788 writer ceiling: below level 3 writers emit the previous full-list shape, byte-compatible with older writers, so a pinned consumer sets `writer_ceiling: 2` (with `writer_ceiling`/`reader_floor` in `ignorable_keys`) and needs no paired move. The observer replay bound is a documented finite 128 MiB with a synthetic 20 MiB event test; other readers were already uncapped. Gate PARTIAL (284 steps, exit 0, no failed steps; 7 delegated-live-scope cross checks skipped-unverified). The 117-second enforcement test's speed is tracked separately in #1844 wrkslots-bounded-evidence-fast-fixture.
