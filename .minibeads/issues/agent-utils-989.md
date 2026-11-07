---
title: 'agentctl-own-draft-submit: drain refuses a composer holding text agentctl itself typed, so every later message waits'
status: open
priority: 1
issue_type: bug
assignee: cl-sync
created_at: 2026-11-07T01:20:00.000000000+00:00
updated_at: 2026-11-07T01:20:00.000000000+00:00
---

# Description

A message to a Claude lane that is mid-turn can end up as unsubmitted composer text that starts with agentctl's own recovery prefix ("may not have reached you:" or "may have arrived:") followed by the queued message. When the lane goes idle, drain refuses: "composer already holds unsubmitted text ...; refusing to append to it; nothing was typed". Every later message to that lane then stays pending until someone presses Enter in the pane.

Seen 3 times in about 20 minutes on one host on 2026-11-06, in two Claude lanes of another harness (reported by its coordinator). It has the same root as #953 coordinator-pane delivery failures: delivery into a busy Claude pane, where the prompt is queued or collapsed and not yet drawn.

# Design

- Before refusing, drain checks whether the composer text is a draft agentctl itself typed: one of its recovery prefixes plus the text of the queued or in-flight message it belongs to (match the message, not just the prefix).
- If it is, submit it with one bare Enter. Never retype it, so it cannot arrive twice. Record the outcome on that message, then continue the queue.
- Any other draft (a foreign or human draft, or a prefix with text that matches no queued message) still refuses, as today.
- Apply the same rule wherever the composer refusal is raised (drain, the watcher's notice delivery, the chat runtime).

# Acceptance Criteria

- A test: a composer holding "may have arrived: <queued message text>" for a queued message is submitted with a single Enter; the message is marked delivered once, and the next queued message is delivered.
- A test: the same for the "may not have reached you:" prefix.
- A test: a composer holding the prefix with text of no queued message, or a human draft, is refused and nothing is typed.

# Notes

Reported 2026-11-06 by the lead coordinator, from another harness's issue tracker. Related: #953 (busy-pane delivery verdicts), 46e81a3b (collapsed paste recognised on the first attempt, never retyped).
