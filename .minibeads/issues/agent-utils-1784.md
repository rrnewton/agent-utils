---
title: 'agentctl-watch-recognise-hook-confirmation: recognise the Claude hook confirmation and page the owner'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-17T15:59:55-05:00
updated_at: 2026-11-17T15:59:55-05:00
---

# Description

[watermelon-full-preview] The watcher reported "Question text is not
recognizable on this screen" for a Claude "Hook PreToolUse:Bash
requires confirmation ... Do you want to proceed? 1. Yes 2. No"
prompt, and the pane froze for 6 h and 8 h with no useful notice.
The hook form has no "Enter to select" footer, so the Claude form
reader fell through to the menu fallback with no question text.

# Design

- Recognise the hook confirmation form (a `requires confirmation`
  heading, `Do you want to proceed?`, numbered Yes/No) as a blocking
  question whose text names the hook and the first line of its reason.
- The watcher's blocked notice then pages a person within one pass,
  naming the pane, the agent and that reason line; and because the
  coordinator cannot answer a hook confirmation, the pass also alarms
  the owner in chat through the bridge's owner path, naming the agent,
  its pane and the first reason line, rate-limited to one alarm per
  prompt per window.
- Never answer it automatically: the answer planner refuses this
  form; a person answers at the pane.

# Acceptance Criteria

- Test with a captured screen: the hook confirmation classifies as a
  blocking question, the blocked finding names the hook and the first
  line of its reason, and no answer plan is produced for it.
- `make validate` passes on the gated HEAD.

# Notes

Filed with #1775 agentctl-drain-composer-wait-labelled-lock-wait,
from the same stuck seats.

## Resolution

[watermelon-full-preview] Landed 4652106e on github/main (gated
562babe4, full validate, 62 steps pass). The Claude hook
confirmation classifies as a blocking question naming the hook and
the first line of its reason in both editions; the watcher pass
alarms the owner in chat through the bridge owner path (agent, pane,
reason, only-you-can-answer), rate-limited to one per prompt per
window, and no answer plan is ever produced for it. Tested with a
captured screen in both editions, plus a pass-level test that the
owner alarm fires once per prompt.
