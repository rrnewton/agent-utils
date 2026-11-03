---
title: 'agentctl-stranded-draft-blocks-delivery: an agentctl-typed draft left in a working composer blocks every later delivery'
status: open
priority: 1
issue_type: bug
assignee: mu-restore2@devbig030
labels:
- agentctl
- agent-teams
created_at: 2026-11-03T06:59:31.858965183+00:00
updated_at: 2026-11-03T06:59:31.858965183+00:00
---

# Description

[opus 5.5] On 2026-11-03 a coordinator's agentctl-delivered messages stopped reaching it for about two and a half hours. At 04:26Z the agent-teams auto-drain (`agentctl drain coordinator --deliver-while-working`) typed a message into the coordinator's working Claude Code pane, where it became a collapsed paste ("[Pasted text #178 +4 lines]") left unsubmitted in the composer. From then on every drain refused with `pane composer already holds unsubmitted text ...; refusing to append to it; nothing was typed` (exit 75, outcome pending): ten agent-teams messages and several agentctl sends piled up, including a P1 request. The auto-drain discards stderr, so the reason was only visible by running the drain by hand. One Enter key press submitted the stranded draft and unblocked delivery.

# Design

- When agentctl itself typed the text sitting in the composer (it recorded what it typed and when), the drain submits it (one Enter) instead of refusing forever, exactly as the watcher's draft sweep already does for some cases; text a human typed is still never touched.
- A composer blocked by unsubmitted text that agentctl cannot attribute is a watcher finding naming the pane, the text's first line and how long deliveries have been blocked, not a silent pending.
- agent-teams auto-drain keeps the drain's stderr (a log file under its state directory) so the refusal reason is visible.

# Acceptance Criteria

- A test leaves an agentctl-typed draft in a working Claude Code composer; the next drain submits it and delivers the queued prompts.
- A human-typed draft is left alone and reported by the watcher with its age.
- An auto-drain refusal reason is readable in the agent-teams state directory.
