---
title: 'agentctl-codex-seats-vanish: agent_not_found while the record says running - the record-vs-reality mismatch goes unnoticed'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-26T19:41:00-05:00
updated_at: 2026-11-26T19:41:00-05:00
---

# Description

Reported by a trusted consumer coordinator (2026-11-26): four Codex seats have vanished since late 2026-11-25 — operations on them answer `agent_not_found` while their records still say running. Several older records are likewise "unreachable while record says running". The agentctl record and the harness reality have silently diverged, and nothing (status, doctor) noticed or reported the divergence; it surfaced only when someone tried to use a seat.

Context that may explain the disappearances themselves, from the same report: one lane's whole scope was SIGKILLed, with oomd pressure or a per-scope limit suspected, on a host that had ample memory at the time; the replacement scope ran without a `memory.max` and peaked at 4.7 GB. Whether the kills and the vanished seats are the same event is not established. The kill's cause is a host/scope question; this issue is the record-keeping one: whatever kills a seat out from under agentctl, the record must not go on claiming running with nothing noticing.

# Design

Make the mismatch visible and self-describing wherever agentctl already reports liveness:

- `status` (and any per-agent read that currently trusts the record) distinguishes "record says running, harness has no such agent" as its own state, named as such (not running, not a plain not-found with no explanation), with when agentctl last actually observed the agent alive if that is recorded.
- `doctor` sweeps records against harness reality and reports each mismatch with the remedy it already has vocabulary for (revive, retire, or remove the record), so a vanished seat is found by the sweep instead of by the next user of the seat. Read-only: doctor reports, it does not rewrite records.
- Operations that today fail with a bare `agent_not_found` on a record-says-running agent say the record and reality disagree and name the doctor/revive next step, instead of reading as though the name never existed.
- Where the harness gives a death signal agentctl can cheaply record at observation time (exit status, scope gone, last-seen timestamp), record it on the record at that moment, so the next mismatch report carries when it was last seen rather than nothing. No new polling loop is implied by this issue; if noticing needs one, that is a follow-up.

# Acceptance Criteria

- With a seat's harness process killed out from under a running record (reproduced by killing the scope/process behind agentctl's back): status reports the mismatch as its own named state, doctor lists the record with a remedy, and an operation on the name explains the disagreement instead of a bare `agent_not_found`.
- A healthy fleet reports exactly as before: no new states, no new findings, no record rewrites from status or doctor.
- The four vanished seats' shape (Codex seats; several older records in the same state) is accounted for in the Resolution: either the sweep reproduces their state and names it, or what differs about them is recorded.

# Notes

Filed 2026-11-26 at the coordinator's direction. The suspected scope SIGKILL (oomd pressure or a per-scope limit; the unconstrained replacement scope peaking at 4.7 GB) is recorded as context only: if the sweep work shows agentctl itself set or omitted a scope limit that invited the kill, that half belongs in its own issue against the scope settings. Related: `#2334 agentctl-revive-archived-journal-mismatch` (same reconcile-the-record theme at revive time).
