---
title: 'muse-bridge-answer-first: ask a bridged agent to answer a chat request before continuing'
status: open
priority: 3
issue_type: task
created_at: 2026-10-29T15:54:23.505853241+00:00
updated_at: 2026-10-29T15:54:23.505853241+00:00
---

# Description

[opus 5.5] From #679 muse-bridge-study. With #678 a chat request reaches a busy Muse within
1-2 s (steered into its turn), but Muse finishes the step it is in before answering: mid-turn
replies came 13-85 s after the post (xhigh slowest; Claude Code 10-16 s). The request prompt
already says replies may be sent at any time, by fence or the `chat reply` command.

Proposal for the owner to decide: a line in the bridge prompt asking the agent to acknowledge
or answer a chat request before continuing its current work, measured before/after on the test
channel. Wording changes the prompt every bridged agent sees, so it needs the owner's OK.
