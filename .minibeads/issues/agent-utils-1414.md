---
title: 'team-restore-driver: a per-team restore driver with a persisted, explicit state machine and an idempotent kick'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-12T11:38:25+00:00
updated_at: 2026-11-12T11:38:25+00:00
---

# Description

[opus 5.5] Restore today is a one-shot command run by a boot unit, by a host-wide wrapper looping over teams one at a time, or by hand. On 2026-11-11/12 that left a team down for 15 hours: the boot run replayed an old report and exited (#1405), nothing retried, and two helpers nearly recovered the same chat bridge at once. Owner, 2026-11-12: one deterministic process must keep the protocol moving even when agents die or misbehave, any human or agent must be able to "give it a kick" harmlessly, and an agent may only get the leftovers. Design: sections 4 and 9 of the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

# Design

- `agentctl team restore-driver --team T`, run as the user unit `agentctl-team@T.service` (restarted by systemd). It is the only process that changes a team's agents during recovery. Mutual exclusion comes from the unit plus an flock on the team's driver state file, with no agent locks.
- States: HEALTHY, DETECT, BOOTSTRAP (certificate, Herdr; once per host per epoch under a host lock), LEAD, AGENTS, UNITS, VERIFY (#1416), HANDOFF, NOTIFY, ESCALATE (#1417). Each has a timeout and retry budget whose expiry moves forward, never into an indefinite wait. State is persisted atomically before each idempotent action, so a crash resumes.
- Epoch = boot id + user-manager identity (#1405).
- `agentctl team kick T`: starts or pokes the unit (idempotent) and prints the state. Without a user manager it runs in the foreground under the same lock. `kick --all` covers every team on the host.
- HANDOFF sends the team lead the remaining checklist and keeps watching. NOTIFY tells every coordinator about the outage and queues each revived lane its resume brief (gates died; re-check landings, rebase, re-queue; nothing heavy under a measurement hold or merge-queue validation, #1406).
- Delivery timebox (lead): a minimal driver (epoch, revive-all from records with the full launch spec, VERIFY, HANDOFF, ESCALATE) may land first once #1415's model of exactly that subset passes.

# Acceptance Criteria

- The driver's transition table is the data #1415 checks.
- Fixture tests: a crash at each state resumes correctly; two concurrent kicks start one recovery; a kick on a healthy team is a no-op that reports healthy; a lead that never comes up escalates.
- The boot unit and the host-wide wrapper become thin kicks (their removal is #1419 team-restore-purge).

# Notes

Numbered from devbig030's interim block (1400-1469). Part of the team-restore workstream; see the team-restore protocol plan (harness repo, `ai_docs/plans/2026-11-12-team-restore-protocol.md`, approved by the lead 2026-11-12).

[watermelon-full-preview] 2026-11-13: the minimal driver landed in the Landing 3 stack — persisted six-state machine interpreting the #1415 table (plus the VERIFY ack-wait rows), per-team flock and `agentctl-team-driver/v1` state, DETECT/restore (replay never trusted when detection found anything missing), VERIFY, HANDOFF, ESCALATE, idempotent `team kick` (unit when a manager answers, foreground otherwise) and `team kick --all`, wake-armed or resident deadlines, host-namespace refusal, and fixture crash/concurrent/healthy/lead-never-up proof. This issue stays open for precisely: (1) splitting BOOTSTRAP/LEAD/AGENTS/UNITS/NOTIFY out of the compound RESTORE step in the model and driver; (2) the installed `agentctl-team@<key>.service` template (host entry point; any namespace entry happens after host-side scope creation); (3) boot unit and host-wide wrapper becoming thin kicks, with their removal as already filed. Nothing else in the acceptance criteria remains.
