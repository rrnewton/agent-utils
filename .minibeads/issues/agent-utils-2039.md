---
title: 'wrkslots-scoped-status-reads-all-events: status --slot parses the whole event log'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T09:00:00-05:00
updated_at: 2026-11-21T09:00:00-05:00
---

# Description

[watermelon-full-preview] Consumer report: `wrkslots status --slot X` took 276 s at load ~150, all of it parsing ~93,527 event files, although only one slot is asked about; their 180 s bound expires with rc 124 and no output. Measured locally on a synthetic valid store of 100,000 events (written through the real writer): `status --slot slot01` rc=0 in 20.5 s, full `status` rc=0 in 17.6 s — the scoped read parses the whole log exactly like the full one (event files are named only by sequence, so which events name a slot is only known after parsing each).

# Design

- The event writer maintains a per-machine index at `EVENT-INDEX.<machine>.json` (control top level; the events directory itself refuses unexpected entries). Content: `{schema, machine, tip {sequence, sha256}, active_revision, slots: {<slot>: {imported, held, hold_released, released}}}` — per slot, the latest sequence of each event kind a scoped status needs besides the ACTIVE snapshot.
- Continuity is proven, never assumed: an index update applies incrementally only when the index tip equals the chain tip the append just extended (previous sequence and sha); otherwise the writer rebuilds the index from a full chain-verified scan, and a failed rebuild leaves the stale index in place for readers to reject. Index maintenance is best-effort inside the append and never fails a mutation.
- Scoped status (`--slot` on one machine) proves freshness before trusting anything: directory listing tip (count + last file digest) equals the index tip, and the ACTIVE snapshot revision equals the index's recorded active revision. Then the slot row comes from the snapshot, and hold state from the at most three sequence-addressed events the index names. Any gap — missing, stale, malformed index, revision mismatch, or release history on the slot — falls back to today's full replay unchanged.
- No event, journal, or lock format changes. Old readers ignore the index file entirely; new readers never trust a missing or stale one, so mixed pins degrade to today's behaviour in both directions.

# Acceptance Criteria

- Scoped status on a 100k-event store reads a bounded number of event files (index + tip + at most a few sequence-addressed events), well under 10 s, with output identical to today's scoped status for the same store.
- A deterministic regression test: scoped status on a large synthetic log does not parse the log (bounded event-file reads, no wall-clock budget), and matches the full status output for that slot.
- Interleaving test: an old-build append (event written without an index update) followed by a new-build append leaves the index continuous and correct for the interleaved slot.
- The existing status/doctor suites still pass; the index is maintained by both event-writing editions or the gap is stated explicitly.

# Notes

Related: #1247 (read bounds and streamed folds), #1931 (writer tip positioning without retaining history), #2040 wrkslots-partial-status-claims-consistent (the partial-status wording this must not regress).
