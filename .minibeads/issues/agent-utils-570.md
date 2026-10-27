---
title: 'agent-progress-monitoring: tell a working-but-stuck agent from a progressing one, and make queued input visible'
status: closed
priority: 1
issue_type: feature
labels:
- agentctl
- observability
created_at: 2026-10-27T03:01:28.208244030+00:00
updated_at: 2026-10-27T04:17:00.166684805+00:00
closed_at: 2026-10-27T04:17:00.166684664+00:00
---

# Description

[opus 5.5] A Codex lane stayed inside one turn for 46 h 54 m. Every message sent to it meanwhile (agentctl send and raw pane input) landed in Codex's 'Queued follow-up inputs' block, which Codex submits only when the turn ends, so none was delivered. agentctl status said working, agentctl send reported a quiet success, and the lane kept writing hourly status lines (activity) while its own status file showed no progress on the goal for about a day. Escape then Enter submitted the queue and the lane took the new instructions within seconds. Another Codex lane hit the same thing twice (about 17 h and 2.5 h). Separately, Herdr reported a Claude Code pane as 'muse, done' after Muse commands ran inside it, so agent lookup by name failed and watch-agents called a working agent unreachable; and a send once said pending when the real cause was agent_not_found.

# Design

1. send reports the real outcome for input that lands in a harness queue (turn age, earlier undelivered messages); status/list show turn age and queued-input counts and ages (harness queue block, agentctl queue). 2. watch-agents alerts on turn age past a threshold (default 2 h) while inputs are queued, with an optional per-agent auto-interrupt-and-deliver policy (off by default). 3. A declared progress metric (file pattern or command) that watch-agents alerts on when it does not move for N hours while the agent works. 4. send --urgent: wait for the harness to take the input at a tool boundary, else interrupt and submit (Codex Escape+Enter, Claude Code queue/interrupt, Muse steering). 5. Classify 'working' from /proc and the screen: model stream silent, tool progressing vs blocked (flock holder, terminal input, network socket, idle child), gate step past its recorded p95, repeated command/error or retry banner, background shell dead or idle. 6. Resolve agents by recorded pane and pinned harness process, not Herdr's detected agent kind.

# Acceptance Criteria

Tests with fake panes and fake /proc trees for every state; captured Codex screens (redacted) as fixtures; gate green; release agentctl built for deployment.
