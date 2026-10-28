---
title: 'agentctl-console: rename tail to console, with an owner input line delivered as a plain user turn'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
created_at: 2026-10-28T18:54:37.529788444+00:00
updated_at: 2026-10-28T18:54:37.529788444+00:00
---

# Description

[opus 5.5] Owner request 2026-10-28 14:53 ET (relayed by another harness's coordinator), verbatim: "Maybe we should rename our tail command console. I would like the option for it to take standard in, so I can paste into the command directly on its terminal. I hope that would work the same as sending it in with an agentctl send. I wonder if this will be treated as prompts directly from me." Design: agentctl console NAME = tail --follow plus an input line; typed or pasted stdin is queued to the headless agent like agentctl send; tail stays as an alias. Deliberate design point: input from the owner's own terminal is delivered as a plain user turn, without agent or bridge sender framing, so the harness treats it as the owner's prompt rather than external content (observed: chat-bridge input sets an external-content session fact, after which owner-directed actions are refused as tainted chat content: 69 of 88 hard refusals for one coordinator). Safety: the console must authenticate the human (same uid, stdin attached to a tty, not a pipe; optionally a confirm keypress) so lanes cannot use it to launder instructions; log every console input with its origin. Verify by inspecting the guardrail plugin's session facts and indicators after a console-typed prompt. Design note first, for owner review.
