---
title: 'agent-teams-core: build the agent-teams tool (P1 model, P3 exactly-once core, P4 ssh route)'
status: closed
priority: 2
issue_type: feature
labels:
- agent-teams
created_at: 2026-10-25T12:32:16.217755995+00:00
updated_at: 2026-10-25T17:55:53.380781522+00:00
closed_at: 2026-10-25T17:55:53.380781392+00:00
---

# Description

[muse watermelon] Build rs/agent-teams per ai_docs/transient/2026-10-24-agentctl-team-messaging.md revision 3: P1 read-only model (teams.json, routes.d with git-ignored *.private.json and AGENT_TEAMS_ROUTES_PATH, declaration.json, show/probe/check/import); P3 exactly-once core (ids, outbox, ledger claim/fence/recovery, accept/receipt/fence/serve, send/status/pump/retract, sender proof with the managed_by tree, local route, Deliverer and Route traits, model-based harness of section 9.7 with make soak-agent-teams); P4 ssh route (use_user_ssh_config permanently, dedicated forced-command keys optional), on-demand and persistent links, star topology with one hub. Live trial between two hosts follows with the coordinator.

[muse watermelon] Landed and live-trialled 2026-10-25. Commits on main: d45f83e1 (tool: P1+P3+P4), f77f0c28 (agent-queue wiring for enqueue and sender proof), 75f7df3a (explicit coordinator agent in a declaration), 137807e4 (designation via agentctl coordinator show), 07321002 (ssh user-config never overrides Control*), 448ba9b8 (local route skipped for other hosts; backup files never walked; truthful attempt detail), 06a720d5 (host state and keys out of synced config trees), f226a2fb (receipt asks never consult skipped routes).

Live trial (one hub host, one spoke host; star topology, spoke dials hub over ssh, hub reaches spoke over a command route; team named in the trial with an entry per host, agentctl-queue deliverer both sides):
1. Hub -> spoke info: sent by the hub coordinator (exit 75), ledger accepted on the spoke 1.6s later, one queue item, typed once by local drain.
2. Spoke -> hub reply: first attempt definite not-delivered (ControlPath bug, fixed above), second attempt misrouted by a synced config file and correctly route-refused with no ledger or queue copy anywhere; after fixes it arrived accepted and typed once into the hub coordinators pane. Settings sync between the hosts was the big environmental finding: the teams file may be shared, host state may not (documented; check warns).
3. Forced ambiguity: a one-shot wrapper swallowed the first accept answer; sender classified ambiguous, settled accepted without a duplicate (one ledger entry, one queue item, typed once). Exposed and fixed: receipt asks consulted a skipped route.
4. Hold/expiry: a 25s-TTL hold deferred by a temporarily broken route was pumped after expiry; receiving ledger recorded expired, nothing queued or typed, sender settled terminally (exit 65).

Exactly-once held through every fault. Known gaps for later phases: sender-side state rests at accepted (no drain-to-ledger delivered sync until the queue lanes phase), retention pruning not yet implemented (entries are kept), relay and chat routes are design-level only, rate limit per_sender_per_hour declared but not enforced, remote retract has no protocol verb yet.

# Acceptance Criteria

P1 and P3 land green with the model harness in the gate; P4 ssh driver lands with tests; CLI quickstart/userguide/help and RELATED_WORK.md present; live trial recorded separately.
