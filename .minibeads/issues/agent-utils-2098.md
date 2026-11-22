---
title: 'agentctl-chat-bridge-write-burst: find and stop the chat bridge''s multi-GB/min write bursts'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-22T06:30:00-05:00
updated_at: 2026-11-22T06:30:00-05:00
---

# Description

A consumer measured one chat bridge (agentctl chat run) as the host's top writer: about 6 GB/min of device writes over a 20 s sample. A later 10 s sample of the same unit showed almost nothing, so it comes in bursts. The bridge state holds several large whole-file JSON stores (short refs, reply aliases, request and reply directories) that may be rewritten whole on each message or pass.

# Findings (owning lane, 2026-11-22)

- Both bridges were idle when re-measured (about 5-10 KB/s over a 10 s per-process I/O sample), confirming a burst, not a steady rate. Persistent state was almost untouched inside the reported burst window (three small commit receipts at the ordinary provider-batch cadence, ordinary rescan passes), so the burst bytes are synced write-and-rename churn, not file growth: every store write is a temporary file plus fsync plus rename plus directory fsync.
- The store layer rewrote whole files with no change check. `short-refs.json` (the largest snapshot; hundreds of KB on a busy bridge) is rebuilt from all retained request records and rewritten whole on every alias assignment and on every one-shot admission; rescan and replay passes, health records and the rescan ledger likewise saved even when their document had not changed. A burst of admissions, alias assignments or replayed batches therefore multiplies whole-store synced writes: at a few hundred such writes a second the arithmetic reaches the reported GB/min. Requests and replies are already per-entry files and were not the multiplier.
- The reply-sender poll writes nothing while idle (measured: the whole bridge averaged well under one write syscall a second while quiet); the many small writes seen over long uptimes are per-event records (breaker, reply and request files, health) that genuinely changed, now also covered by the change guard.

# Design

- The store layer (`write_document`) skips a write whose encoded document is already the file's content, so no loop or pass rewrites state without a change.
- `short-refs.json` is coalesced: alias assignments and admissions only note that it is behind, and the service rewrites it once per owner-loop pass (and on shutdown), so a burst costs one snapshot write per pass, not one per event. Ordering is preserved for other processes that read only the files: a prompt flushes the file (when behind) before the prompt carrying a new reference leaves the bridge, and opening the state notes the derived file behind so the first prompt or pass after a restart rebuilds it. The file and its `agentctl-short-refs/v1` format are unchanged, and no persisted file or field is added, so older builds read the state as before.
- The reply-alias record stays a single durable file, written only on change (assignment, burn, withhold) as before: it must be durable before a prompt shows the alias, and assignments are request-paced, so it cannot multiply at machine rate. The Slack rescan ledger gets the same skip-if-unchanged guard.

# Acceptance Criteria

- A test drives N messages and a rescan through the bridge and asserts the bytes written grow with the change, not with the store size; the burst cause is named.

Covered by `store_writes_scale_with_the_change_not_the_store_size`: it prefills two stores (32 and 256 requests), then asserts through the filesystem that a no-change rescan writes 0 bytes, one admitted message costs only its own records (less than the snapshot size, and the same beside the eight-times-larger store), and one alias flush rewrites the snapshot exactly once.

# Notes

Filed by the coordinator from a consumer's report; the owning lane refines this file as it works.
