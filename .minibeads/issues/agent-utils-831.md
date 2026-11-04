---
title: 'chat-dead-pin-reanchor: re-anchor or name the fix when a pinned harness died and a live agent holds the pane'
status: closed
priority: 1
issue_type: bug
assignee: cl-start@devbig030
labels:
- agentctl
created_at: 2026-11-03T22:00:33.302217208+00:00
updated_at: 2026-11-04T00:07:34+00:00
closed_at: 2026-11-04T00:07:34+00:00
---

# Description

[opus 5.5] Root cause of the 2026-11-03 warning spam (#830): a coordinator was restarted, but the bridge's registry record still pinned the dead launcher process, so every pass refused input ("refusing input to agent ...: the anchor ...") while the warning only said "not reached it". `agentctl anchor --replace` fixed it at once.

# Design

When a pinned harness process is dead but the same pane now runs a live agent of the same kind, either re-anchor automatically (with a journal line saying so), or at least make the warning name the reason and the exact fix command. Never loop silently.

# Acceptance Criteria

- A test with a dead pin and a live same-kind agent in the pane yields either delivery (after an automatic re-anchor, journaled) or a warning that names the reason and the fix command.

# Notes

## Resolution

[opus 5.5] Delivered as the actionable warning, not an automatic re-anchor. Anchoring exists to tell a replaced agent apart, and only an operator can confirm that a new process is the same agent.

- When the anchored harness fails its foreground check and the pane now runs another process of the same harness, the refusal names both processes and the exact fix: "refusing input to agent 'NAME': the anchored claude process (pid A) has exited, and pane P now runs another claude process (pid B); if it is NAME, run `agentctl anchor NAME --replace` to pin it (look at the pane first: `agentctl attach NAME`), and delivery resumes". If the new process's command line resumes the record's native session, the refusal adds "which resumes NAME's recorded session S, so it is most likely NAME restarted". Every other case keeps the old wording. Nothing is typed in any case.
- That reason reaches the request's pending detail, as "request remains pending: …". The #830 owner alarm quotes it under "Why:" and adds a hint naming `agentctl anchor NAME --replace`. The watcher's held-queue escalation (#834) gives the same hint.
- Tests:
  - `a_restart_resuming_the_recorded_session_is_named_as_likely_the_same_agent` (identity): a dead pin plus a live same-kind harness gives the actionable refusal, then delivery after `anchor --replace`.
  - The updated `a_harness_replaced_before_send_types_nothing_and_stays_pending`.
  - `a_restarted_harness_warning_names_the_anchor_command` (owner alarm).
- A journaled automatic re-anchor when the session matches would be a separate issue if wanted.
