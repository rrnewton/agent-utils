---
title: 'agentctl-muse-stale-draft: a draft agentctl typed but Muse never submitted holds every owner message'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-01T08:07:37.884533520+00:00
updated_at: 2026-11-01T08:07:37.884533520+00:00
---

# Description

[opus 5.5] A chat bridge's owner messages to a Muse coordinator stalled for about an hour behind one agent message that agentctl had typed but Muse never submitted.

What happened: `agentctl send` typed an agent's message into a working Muse coordinator during a turn. The screen showed it as a steered user turn (the proof agentctl took), and the queue marked it processed. Muse then compacted its context, the steer failed, and Muse put the prompt back in the editor as an unsent draft. Every later delivery was refused: a working pane "does not show an empty Muse editor", and an idle one reported `staged`, which the readiness check never accepts. Eight owner requests waited up to an hour. Nothing alerted anyone, the agent kept looking "working" because scheduled turns kept firing, and a "PROBABLE MISROUTE" quarantine of one owner request reached nobody. Pressing Enter in the pane by hand cleared it at once.

Wanted:
1. Verify every Muse submit: a screen-only proof (a user turn, a queued input) that Muse later takes back into the editor is not a delivery; press Enter again, within bounds.
2. Recover stale drafts: when a delivery finds the editor holding a draft that agentctl itself typed (its sender header, matching a record in this agent's queue), submit it and carry on. Never submit or clear a draft agentctl did not type; report it.
3. Alarm on stuck owner messages: a chat request left undelivered past a threshold (default 5 min) is reported to the owner by the bridge and to the coordinator by watch-agents, with the concrete reason, and repeated until it clears.
4. Owner priority: an agent message stuck in the editor blocks owner messages no longer than that recovery takes.
5. Quarantine notices: a PROBABLE MISROUTE quarantine notifies the coordinator, and the owner for chat requests, with what was quarantined and how to send it again.
