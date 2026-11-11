---
title: 'console-follow: agentctl console shows no agent output, only queued lines'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-11T15:58:51.144549282+00:00
updated_at: 2026-11-11T15:58:51.144549282+00:00
---

# Description

[watermelon-full-preview] Owner bug, urgent (owner reply 370, 10:46 AM ET, 2026-11-11): running `agentctl console coord7` against a healthy headless Claude Opus agent (devbig030, snapshot 224316b4) prints ONLY `queued <id>` lines separated by blank lines, prompt `coord7 ›`. No transcript, no startup banner, no /help. Owner: 'I DONT have the prior tail output that I expected to see with a detailed log of whats happening on the agent. This is like a black hole. Whats the design here!?' Design intent is #670 agentctl-console (rs/agentctl/src/console.rs module doc): console = tail --follow rendering of the agent events PLUS an input line. The follower output is missing. Live evidence from coordb on devbig030: (1) only queued lines + blank lines, no banner; (2) /help is NOT handled locally, it is queued as a user turn, as is any other text; (3) Ctrl-C on empty line arms interrupt as designed; (4) queued lines show no position or ETA, ~9 turns pending; (5) agentctl cancel refuses headless agents (separate follow-up, see Notes).

# Design

- Console must show exactly what tail --follow shows (assistant text, tool calls, results, state changes) and those lines must PERSIST above the input line; the startup banner must show at start.
- Follower-thread errors must be printed in the console and the console must say when the follower died, never swallowed (lead: console.rs let _ = tail::run(...) in a thread).
- Local commands: /help lists /interrupt, /quit and keys and is never queued; every unknown /command is refused locally with the /help hint, never queued; plain text continues to queue as a plain user turn per #670.
- Each queued line shows its queue position and the running turns age.
- PTY tests: a turns assistant text and a tool line appear and survive input-line redraw; a follower error is shown. No wall-clock budgets: wait on events/conditions with generous timeouts (owner rule 346).

# Acceptance Criteria

- PTY reproduction on a scratch headless agent in a scratch registry shows console rendering the same assistant/tool lines as tail --follow on the same agent, persisting above the input line, with banner visible at start.
- /help is answered locally and never queued; an unknown /command is refused locally with /help hint and never queued; plain text still queues.
- A queued acknowledgement shows queue position and running-turn age.
- A follower failure is printed in the console and follower death is announced.
- Maintained PTY tests cover turn visibility surviving redraw and follower-error surfacing; make validate passes on the gated tree.

# Notes

Filed by mu-consolefix (slot gatefast, branch mu-consolefix/console-follow) at coordb's instruction for owner reply 370. Related: #670 agentctl-console (design intent), #1312 headless-cancel-queue (separate follow-up for cancelling queued headless turns, filed in the same tree). Coord7 on devbig030 runs snapshot 224316b4 and is healthy; do not touch real registries while reproducing — use a scratch registry and scratch headless agent.
