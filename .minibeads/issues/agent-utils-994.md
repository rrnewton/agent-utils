---
title: 'wrkslots-archived-record-stopped: an archived agentctl record is treated as possibly live and blocks removal'
status: closed
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-07T02:05:00.000000000+00:00
updated_at: 2026-11-09T07:20:00.000000000+00:00
closed_at: 2026-11-09T07:20:00.000000000+00:00
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

Same deadlock family as #400 slot-reclaim-deadlock (an archived herdr-relay agent whose slot nothing may remove). The refusal text comes from the consumer's own liveness probe, not from agent-utils. The lead is routing that probe fix to the consumer's team. Scope here, per the lead (2026-11-08): option (a) only. Widening `--attest-agent-stopped` to archived records (b) was rejected: it would weaken the one place where attestation is narrowly allowed.

## Resolution

Delivered by cl-wrkid, 2026-11-08 ET (option (a)), in both editions.

- Example probe (py/wrkslots/examples/agentctl_liveness_probe.py): a dead verdict names its evidence after the reason: `reason=stopped-by-agentctl archived=<newest matching archived record's directory> archived_at=<when that agent.json was last written, UTC>`. agentctl writes the record before it moves it into the archive, so that time is the archive time, to within the stop.
- wrkslots records the line. Every agent removal whose registered liveness command answered dead writes one active-state-recorded mutation: action `registered-liveness-recorded`, the record unchanged, and evidence `{generation, agent, registered_liveness: "dead", detail, detail_truncated}`.
  - It is written inside the existing registry-lock hold, before salvage, like the owner-consent, release and attested-stop records, so it adds no extra lock round trip (#1108).
  - The detail is the authority's first line, the same line wrkslots already takes. It is never parsed. Control characters are escaped as `\xNN`, it is capped at 1024 UTF-8 bytes, and `detail_truncated` says when it was cut.
  - Removals under an owner consent or release keep recording the authority's answer in their own evidence, as before.
  - Why an action and not a journal key or event field: the FINISH journal and the reclaim-started payload are parsed with exact keys by both editions and by older pins, so a new key there would be a flag day. Every reader, including the wrkslotsd replay, accepts any active-state action whose evidence is an object.
- Rust parity (the lead approved option (A)): rs/wrkslotsd/src/remove_write.rs writes the same mutation in its locked step, and its dead detail is now the line Python records (standard error when it is not empty, else standard output, stripped, first line), so the remove differentials stay exact.
- Tests:
  - py/wrkslots/tests/test_agentctl_liveness_probe.py: the newest archive and its time are cited;
  - py/wrkslots/tests/test_lifecycle.py: the line is recorded, and a long line with a control character is escaped, one line, 1024 bytes, and flagged truncated;
  - py/wrkslots/tests/test_remove_write_rust.py: both editions record identical evidence for such a line; the existing differentials (ACTIVE document, event kinds and actions) still match.

