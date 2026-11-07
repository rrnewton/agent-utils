---
title: 'wrkslots-attest-archived-pane: no way to attest a stopped agent whose archived record names a reused pane'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-07T22:50:00.000000000+00:00
updated_at: 2026-11-07T22:50:00.000000000+00:00
---

# Description

`wrkslots remove` refuses a slot whose owner is dead when the liveness authority answers "unverifiable ... reason=archived-record-pane-still-occupied". The agent's agentctl record is archived, but the pane it names is now used by a different agent, so the check cannot prove the old agent stopped. `--coordinator-authorized --attest-agent-stopped NAME --attest-note TEXT` covers only an agent with no record, so it does not apply. The slot can never be reclaimed.

# Design

- Let `--attest-agent-stopped` also cover `archived-record-pane-still-occupied`, recorded the same way.
- Better: have the liveness check compare the pane's current agent (or the process tree's identity) with the archived agent, and report the old agent stopped when the pane now runs someone else.

# Acceptance Criteria

- A dead-owner slot with an archived record and a reused pane can be removed: automatically if the pane provably belongs to another agent, otherwise with an attestation.
- Tests for both paths.

# Notes

Two finished slots (team-restore work, done 2026-11-02) are stuck this way as of 2026-11-07.
