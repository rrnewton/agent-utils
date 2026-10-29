---
title: 'agent-turnover: one named agent, a lineage of sessions, and a deterministic handoff-and-replace command'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
created_at: 2026-10-29T13:20:59.460112161+00:00
updated_at: 2026-10-29T13:20:59.460112161+00:00
---

# Description

[opus 5.5] Owner request (2026-10-29): an agentctl named agent should be able to span several harness sessions. Sometimes a restart resumes the same session ID (revive); sometimes the harness is broken or the context is exhausted and the right move is a fresh session seeded from a handoff. agentctl should record that continuity and be able to perform the turnover deterministically.

Today: the agent record holds one native_session; revive replays the launch spec and conversation; stop archives the record under <name>-<time>-<pid> in archive/; name_history/former_names track renames, not session replacement. Nothing links an archived record to its successor.

Wanted:
- A session lineage on the agent record (its own subdirectory if it grows, per the O(N) rule): each entry has harness, session id, started/ended, how it began (fresh, resumed, from-handoff, takeover), the handoff it was seeded with, and why it ended.
- agentctl turnover NAME [--fresh|--resume] [--handoff-file PATH] [--profile P]: (1) ask the running agent for a handoff using a standard template, naming where to write it; wait for the file plus a verdict marker, with a timeout; (2) stop the old session, keeping its queue; (3) start the replacement with the same name, slot, profile (or an override, e.g. switching harness) and registry identity; (4) feed it the handoff as its first prompt; (5) carry over queued messages and slot ownership (wrkslots transfer-owner or adopt with the next generation); (6) append to the lineage; (7) notify the coordinator. --resume uses revive-style resume when the session is healthy.
- agentctl history NAME: the lineage, with links to each session's transcript.
- The watcher and status show 'turned over at T (reason)' instead of treating the new session as an unrelated agent.
Why continuity matters: messages addressed by name keep working; slot ownership and landing queues (hold --after) survive; audit and cost per lane span sessions; context-window rollover becomes routine; cross-harness switches (e.g. one harness to another) keep the lane identity.
Acceptance: turn over a live test agent mid-task (fresh-from-handoff and resume variants) with no lost queued message and the slot still owned; lineage and history shown.
