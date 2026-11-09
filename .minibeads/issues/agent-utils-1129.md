---
title: 'restore-registry-derived: restore every agent running at shutdown instead of a hand-kept include list'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T13:45:32.659440213+00:00
updated_at: 2026-11-09T13:45:36.691504180+00:00
---

# Description

Found during the 2026-11-09 host reboot. One registry's restore.json listed only two agents as desired state, so none of its ten working lanes came back; the coordinator revived them by hand. A second registry kept a long hand-maintained include list that had drifted from what was actually running.

The per-registry restore treats the agents key, when present, as an allowlist: every registered name not listed is excluded as not in desired state, however recent its running record is. The include list must be kept fresh by hand (or by a separate write-agents step nobody ran at shutdown), so a reboot restores the list, not the team that was running.

Impact: after an unplanned reboot, the lanes doing the work stay down until a person notices and revives each one. The exclude list already expresses names to leave alone; the include list duplicates that decision in a form that goes stale silently.

# Design

Make restore registry-derived by default: restore every agent whose record was running at shutdown (lifecycle running and anchored to a previous boot, or otherwise provably live at shutdown by the same evidence the write-agents preview uses), coordinator first, managers before their managed agents. The explicit exclude list is the only hand-kept control; a name in exclude is left alone however live it was.

Keep the agents key only as an explicit opt-in pin for registries that want a closed desired set, and name that choice in the report (for example a desired-state source field: registry-derived or pinned-list) so an operator can tell why a lane was excluded. When no agents key is present, absence means registry-derived, not restore-nothing. The write-agents preview keeps its record-versus-live mismatch refusal; it becomes a way to materialise a pin, not a step required for a correct reboot.

Finished, stopped, and archived records are never revived by derivation. A record whose liveness at shutdown cannot be decided is refused with its evidence, not silently excluded.

# Acceptance Criteria

- A registry with no agents key, ten records running at shutdown and two stopped, restores the coordinator first and then the ten running records; the stopped records are reported as skipped, not revived.
- A name in exclude that was running at shutdown is reported as excluded and is not revived; the coordinator can never be excluded (validation keeps refusing it).
- A registry that still pins an explicit agents list restores exactly that list plus the coordinator, and its report names the pin as the desired-state source.
- A record whose shutdown liveness is undecidable is reported as refused with its evidence and counts toward exit 74, never as a silent exclusion.
- Existing restore suites that pin agents keep passing; new coverage exercises the derived path with no agents key.

# Notes

Scope gap (a) from the 2026-11-09 reboot restore. The two-agent include list and the ten missing lanes are from this repository's own team registry; the long drifted list is consumer-a's. Touches the per-registry restore desired-state selection in both agentctl editions and the cross differential; coordinate with the restore-core lane to avoid overlapping edits in the same functions.
