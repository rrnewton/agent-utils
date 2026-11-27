---
title: 'agent-teams-orc-front-door: an Orc-run team coordinator needs an idempotent inbound deliverer and a delivered outbox'
status: open
priority: 1
issue_type: bug
labels:
- agent-teams
- agentctl
- coordinator
created_at: 2026-11-26T19:35:00-05:00
updated_at: 2026-11-26T19:35:00-05:00
---

# Description

In the Orc subteam trial (#2108) the team's coordinator is an Orc session adopted into agentctl with `--harness unknown`, and agent-teams messages to that team did not work in either direction.

Inbound: the team declaration delivers through `agentctl-queue` to the registry's designated coordinator, and the designation still named the previous agentctl coordinator. Every team message from 2026-11-24 to 2026-11-26 was accepted by the receiving host in about 0.2 s and then held at the receiver ("composer already holds unsubmitted text ... nothing was typed") behind an owner draft in the previous coordinator's pane, until its time to live ran out. The Orc coordinator never saw them, and the sender only saw `accepted` / `delivery_outcome: pending` (see #2326). Designating the Orc record (2026-11-26) made inbound delivery work: a test message was typed into the Orc pane at once and answered. But that relies on typing into a TUI composer, and designation has side effects: watch-agents and restore now treat the Orc record as the root coordinator, and restore cannot revive an unknown-harness coordinator (it has no launch spec).

Outbound: an Orc script cannot pass agent-teams' sender proof (process ancestry must contain the designated coordinator's harness), so the coordinator can only write to an outbox spool. No service drains it: a blocked request to the lead coordinator sat unread for two days, and the reply to the 2026-11-26 round-trip test is still only on disk.

The trial plan's section 3.5 named both pieces (a command deliverer or designation, and a spool drained by a registered service). Neither was built before the bridge was pointed at the Orc session.

# Design

- Inbound: a deliverer kind for Orc targets (or the existing `command` deliverer with a small helper) that submits the message through the Orc session's local HTTP API (`POST /api/message`, body field `message`) with an `Idempotency-Key` derived from the agent-teams message id as a name-based UUID, so a retry or a second route submits it once. Orc queues a message that arrives mid-turn, and the queued message survives `orc session restart` and a hard stop plus resume (measured on a scratch session, #2329). Delivered means a 2xx answer with an `input_id`, which the ledger records. This keeps the registry's designation free for restore.
- Outbound: the registered-service path from the plan: a user unit drains the coordinator's outbox spool with `agent-teams send --from-service <service> --file F --kind K`, moves each file to `sent/` or `refused/` with the result, and never sends one file twice.
- Checks: `agent-teams check` flags a team whose deliverer target is not its live front door (the designated record's pane is not the coordinator the chat bridge targets), and any receiver-held delivery older than 30 minutes. A designation change records the watch and restore side effects in its output.

# Acceptance Criteria

- Against a fake Orc HTTP endpoint: a team message is submitted once; a retry with the same message id yields one input; a busy target queues it; an unreachable target defers and retries; the ledger records the input id.
- The outbox drainer sends each spooled file once, records refusals, and restarts cleanly mid-drain without a duplicate.
- An end-to-end round trip on a scratch Orc session, lead to Orc team and back, delivered once in each direction.
- `agent-teams check` reports the stale-designation and long-held cases with their fixes.

# Notes

Found by the #2108 debugging pass for owner request 392 (2026-11-26). Interim state on the trial host: the Orc record is designated (revert recorded with the change), 42 stale held team messages were retired from the previous coordinator's queue, and the outbox still has no drainer. Related: #2326 (sender cannot see the held reason), #2329 (Orc restart and resume behaviour).
