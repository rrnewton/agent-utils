---
title: 'wrkslots-schema-level-version-skew: a newer writer bricks every older reader on the append-only log'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T12:51:55-05:00
updated_at: 2026-11-17T12:51:55-05:00
---

# Description

A wrkslots reader refuses records that a newer wrkslots wrote on the same host. The event log is append-only, so one newer write blocks every older reader for good. Seen with a newer writer and an older pinned reader on one host: a read-only classify command failed with "REFUSED: append-only active-record event.record.handoff has invalid fields: unknown ignored_archives, ignored_files" (exit 3) under the older pin and succeeded (exit 0) under the newer build. The additions involved were handoff accounting fields and a seal-journal filesystem id. Cause: the strict key-set check treats any unknown key as corruption, so an additive optional field is indistinguishable from damage.

Prior art: the archive-only fix made archived-row readers tolerate unknown shapes monotonically and quarantine one bad row instead of refusing the registry. The agentctl forward-compat rule tolerates unknown keys in persisted files, warns once, and keeps keys through rewrites; versioned agent-record schemas were handled the same way. Journals already have an extension slot whose minimum-version field makes an older reader refuse loudly by name, and the project configuration already has a minimum-version floor plus an ignorable-keys list. None of that covers events, the active record, the handoff, seals, receipts or manifests: those still refuse on any unknown key.

# Design

Approved design (coordinator review): decision-relevance is decided per command at the point of use, not at history load. Load and replay keep every record with unknown keys preserved raw and warn once; only remove/finish of the affected slot refuses, with a typed NEWER WRITER refusal naming the slot and fields, distinct from "invalid fields" corruption. Records carry the writer schema level via the project gate: `.wrkslots.yml` gains `reader_floor`/`writer_ceiling` (raised deliberately only with init --repair), and a writer never satisfies the ceiling with a less safe older form — handoff ignored-file accounting and its summary replacement refuse above the ceiling, the seal filesystem id is omitted (anchor fallback), and a finish journal keeps its containment list inline instead of a sidecar reference. Active-record/event fields are permanent (replay revalidates them); journal-only fields are transient. `wrkslots doctor` reports the floor, ceiling, and records newer than the floor. Both editions in differential parity.

# Acceptance Criteria

- Old-reader fixtures read new-writer events, journals and handoffs: additive optional fields a reader can ignore for the decision at hand are accepted; decision-relevant unknowns refuse with a typed "newer writer" refusal naming the writer version and the minimum reader version, distinct from "invalid fields".
- A writer does not raise the schema level past what the host's readers accept unless the project allows it (floor and ceiling in the project configuration, raised deliberately with init --repair).
- A version-skew regression test covers both directions in both editions: an old-reader fixture reading new-writer events, journals and handoffs fails before the fix and passes after.
- The deterministic regression tests use condition-based waits only; broad validation runs the full local validate plus the gate on the exact final tree.

# Notes

Coordinator confirmations (on c4e83d0c): (1) When `reader_floor`/`writer_ceiling` are unset, defaults are floor 1 and ceiling this build's schema level (2), so every field written today is at or below the ceiling: nothing is refused, omitted, or changed for any existing project; the gate only bites after a project deliberately sets a lower ceiling or a future field exceeds it. (2) The summary-plus-manifest replacement for the per-file ignored list is in the per-field writer table at the same level as the accounting it replaces: under a ceiling below that level the finish refuses rather than falling back to the unbounded per-file list, and any writer of the summary must assert the level first. Unknown-key presence stands in for a stamped level integer, because stamping a new top-level key would itself brick already-deployed readers.
