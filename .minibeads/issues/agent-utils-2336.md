---
title: 'wrkslots-liveness-for-orc-lanes: a liveness probe that knows Orc lanes, so their slots are removed without attestation'
status: open
priority: 2
issue_type: feature
labels:
- wrkslots
- agentctl
created_at: 2026-11-26T19:45:00-05:00
updated_at: 2026-11-26T19:45:00-05:00
---

# Description

wrkslots asks the harness's liveness command (`bin/agent-liveness`, which reads the agentctl registry) whether a slot's agent has stopped before it removes the slot. Lanes that an Orc coordinator spawns (Orc trial #2108) are Orc agents running in panes of Orc's own tmux server, not agentctl records. For them the probe answers `unverifiable ... reason=no-agentctl-record`, and a plain `wrkslots remove` refuses: "unknown ownership is not a free slot". On 2026-11-26 the first real Orc-lane slot could only be removed by a new attested path in the trial team's wrapper (`remove --coordinator-authorized --attest-agent-stopped LANE --attest-note ...`). That path also needs the lane to have adopted and released its slot first, because an attested remove refuses a slot whose owner was never bound. Every Orc-lane slot would otherwise pile up, and attestation is a human-grade escape hatch where a probe could give the answer.

# Design

- Teach the liveness probe a second authority for agents it does not find in agentctl: a configured Orc session (or every live Orc session of the user). It asks the session's local API or `orc session info` for the agent list and answers dead (verified) when the session is live and lists no agent of that name, or lists it as closed; alive when it lists it as running; unverifiable when the session cannot be read. Keep the one-line `agent=NAME rc=N` output contract.
- Cross-check with the process view the probe already uses: no live process of the user may still run with the lane's tmux pane or cwd inside the slot.
- Configuration names the Orc session(s) in the project's wrkslots or harness config; no host names in code.

# Acceptance Criteria

- Tests with a fake Orc API: a closed lane answers dead, a running lane alive, an unreadable session unverifiable, and an agentctl record still wins when both know the name.
- With the probe configured, a closed Orc lane's slot is removed by a plain `wrkslots remove` (no attestation), and a slot whose lane is still running is refused.
- The probe's documentation lists the Orc authority and its failure modes.

# Notes

Found in the #2108 debugging pass (owner request 392, 2026-11-26). Workaround in the trial team's wrapper: `slot-remove-closed` (attested). Related: #2332 (a failed create's journal blocked removals).
