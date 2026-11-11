---
title: 'agentcloud-send-steer: steer agentcloud input into the running turn and show it queued'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-11T09:41:59.329712491+00:00
updated_at: 2026-11-11T09:42:07.755165146+00:00
---

# Description

Sending input to an agentcloud-adapter agent journals it with end_of_turn set, so the input waits for the lane's current turn to end. A lane that stays in one long turn (for example re-arming hour-long wait calls on a gate) never sees it: on 2026-11-11 a landing authorization sat for about two hours behind such a turn. The urgent flag does not help (it is documented as interactive-only), and interrupt refuses agentcloud agents (it applies to headless agents). The working path today is calling the cloud session tool directly without the end-of-turn marker, which steers into the running turn. Nothing in status or the watcher shows that input is queued behind a running turn, so the stall is invisible until someone reads the journal by hand.

# Design

Three parts. (1) Steering send: the urgent send (or an explicit steer mode) for agentcloud agents maps to the cloud session tool's mid-turn steer (no end-of-turn marker), through the same durable queue and delivery accounting as an ordinary send; an ordinary send keeps end-of-turn semantics. The help text stops calling urgent interactive-only and states per-harness behaviour. (2) Visibility in status: status for an agentcloud agent with undelivered input queued behind a running turn reports that state with its age (queued since a stated time), read from the journal and live turn state, without delivering anything. (3) Watcher coverage: the agent watcher flags an agentcloud agent whose input has sat behind a long turn past the same thresholds it already applies to interactive agents, routed to the manager or coordinator as today, once per episode. No behaviour changes for interactive or headless agents.

# Acceptance Criteria

- With a fake agentcloud adapter, an urgent (steer) send is recorded as a mid-turn steer with no end-of-turn marker, while an ordinary send keeps the marker; both go through the durable queue exactly once.
- Status for an agentcloud agent with input queued behind a running turn reports the queued-behind-turn state and its age; with no queued input it reports none.
- The watcher raises one notice per episode for an agentcloud agent past the queued-behind-turn threshold and does not repeat it until the input delivers or the turn ends.
- Interactive urgent delivery and headless interrupt behaviour are unchanged; existing send, status, and watcher suites keep passing in both editions.

# Notes

Filed on coordinator instruction 2026-11-11 after a landing authorization waited about two hours behind a long turn. Issue-only for now; implementation follows the restore batches. The direct cloud-tool steer remains the operator workaround until this lands.
