---
title: 'wrkslots-archived-record-stopped: an archived agentctl record is treated as possibly live and blocks removal'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-07T02:05:00.000000000+00:00
updated_at: 2026-11-07T02:05:00.000000000+00:00
---

# Description

Removing an agent slot whose agent's agentctl record is ARCHIVED refuses with "archived agentctl record <name> (adapter herdr-relay): runs its agent outside a local herdr pane, so local absence is not evidence of death".

An archived record means `agentctl stop` (or a retire) already retired that agent. It should count as stopped, with the archive timestamp as evidence, whatever the adapter. Today the relay adapter's rule ("the agent runs elsewhere, so absence here proves nothing") is applied to the archived record as if it were live. Neither `--coordinator-authorized` nor `--attest-agent-stopped` is accepted for it, so only the owner's override gets through.

# Design

- When the only record for the slot's agent is archived (and no live record of that name exists in any configured registry), treat the agent as stopped. Cite the archive path and timestamp in the liveness evidence and the journal.
- A live record of the same name, or a newer generation, still takes precedence and is checked as today.
- Both editions, and the liveness helper the harness configures, if it makes this decision.

# Acceptance Criteria

- A test: a slot whose agent has only an archived herdr-relay record is removable, and the output names the archived record and its timestamp as the evidence.
- A test: an archived record plus a live record of the same name still applies the live record's rules.
- Both editions agree.

# Notes

Reported 2026-11-06 by the lead coordinator from a consumer's cleanup lane. Related: #993, #954-#956.
