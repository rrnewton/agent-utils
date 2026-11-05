---
title: 'revive-abandon-dead-candidate: an interrupted revive whose candidate died no longer wedges the agent'
status: open
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T17:13:09.823347188+00:00
updated_at: 2026-11-05T17:13:09.823347188+00:00
---

# Description

An interrupted revive whose candidate never came up wedges the agent for good. After a load spike on 2026-11-05, `agentctl revive <name>` for a Codex lane in a consumer team's registry opened a candidate pane, but the harness never started (the pane shows no agent, probably killed under load). Every later revive refuses:

    interrupted revive candidate is not independently verified; refusing another launch

The revive journal (`.agentctl/.revives/<id>/`) still holds the interrupted attempt, and nothing was hand-edited. There is no supported way out. The agent's 4 queued prompts wait behind it.

# Design

- **Resolve a dead candidate from evidence.** A revive that finds an interrupted candidate checks it independently. If the candidate pane is gone, holds no harness process, or its harness process is dead, the attempt is marked failed in the journal, with the evidence recorded. A new launch then proceeds.
- **Automatic only when unambiguous.** When the evidence is unambiguous (the pane is gone, or no process in its tree is a harness), revive does this by itself. When it is ambiguous (for example a live process whose identity cannot be checked), revive refuses as now, but names the explicit way out.
- **Explicit override.** `agentctl revive NAME --abandon-candidate` marks the interrupted candidate failed after the same checks, refusing if a live harness process is in the candidate pane, and then relaunches. Document it in help and the userguide.
- **Queue survives.** Queued prompts are kept and delivered to the revived agent as usual.

# Acceptance Criteria

- Test: an interrupted revive whose candidate pane has been closed. The next revive marks it failed, with the evidence in the journal, and launches.
- Test: the candidate pane is alive with no harness process. The same result, automatically.
- Test: the candidate pane holds a live harness process. Revive and `--abandon-candidate` both refuse, naming the process.
- Test: queued prompts are delivered after the successful revive.
- Live check (through the lead, or the scoped #896 route while it exists): the wedged consumer-team record revives, with its 4 queued prompts kept.

# Notes

Reported by the lead on 2026-11-05 at 12:12 PM ET, from a consumer team's coordinator (P2, ranked after #896). The wedged record and its journal are kept as evidence in that team's registry on this host. They are read-only for us; ask through the lead for any test against them. Assigned to cl-start after #896 and #897.
