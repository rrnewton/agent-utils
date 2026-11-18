---
title: 'wrkslots-create-recovery-ttl-drift-loop: a config heartbeat-TTL edit strands create recovery in a refusal loop'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-18T00:05:24.000000000-05:00
updated_at: 2026-11-18T00:05:24.000000000-05:00
---

# Description

Crash a create, then edit `heartbeat_ttl_seconds` in config: `recover` (including `--abort-create`) raises `StateError: create journal heartbeat TTL differs from configuration` (py/wrkslots/cli.py:68801; the equality check precedes both the durable-row clear and the abort branch at :68859). `quarantine-journal` does not rescue it: for an incomplete journal classification succeeds without the TTL check, so quarantine refuses as recoverable and points back at recover (:14655 region); for a completed journal the same `StateError` (:12953) propagates through classification uncaught. Create of the same slot refuses on the outstanding journal. The only exit is reverting the config value to the journal's recorded one — a remedy no message names, and a deliberate fleet-wide TTL change must be temporarily un-done per stranded create. The eden create twin has the identical check (:41541). Found by exhaustive search in #1828 wrkslots-transition-model-check (report section 4.2; minimal trace CrJournal, crash, config edit).

# Design

Follow the hooks precedent already in the same validation path: "the journal's recorded hooks govern, even when the current configuration has since changed them; drift is an audit note, never a refusal" (py/wrkslots/cli.py:12941). Apply it to the recorded TTL for recovery, abort, and quarantine classification — the journal value is what the slot was created under; drift is reported, not refused. At minimum, exempt the abort path (it publishes nothing) and make quarantine classification survive the mismatch, and name the config-revert remedy in any refusal that remains. Cover the eden twin identically.

# Acceptance Criteria

Deterministic tests: create crash, edit config TTL, then (a) recover completes the create with the drift noted, (b) recover --abort-create rolls it back, (c) quarantine-journal dry-run classifies without dying on the mismatch; completed-row and eden variants covered. Tests fail before (StateError / refusal loop) and pass after; condition-based waits only.

# Notes

[watermelon-full-preview] Filed from #1828 wrkslots-transition-model-check at the coordinator's request; see ai_docs/reports/research/2026-11-18-wrkslots-model-check.md section 4.2.
