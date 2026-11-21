---
title: 'agentctl-answer-dangerous-confirm: require --confirm-dangerous to answer a dangerous-operation permission prompt'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-21T10:01:00-05:00
updated_at: 2026-11-21T10:01:00-05:00
---

# Description

[watermelon-full-preview] Since #2011
agentctl-claude-permission-answer, `agentctl answer` can answer
Claude permission prompts, including the 'Dangerous rm operation'
warning form; the pin that used to refuse those is gone. Nothing
answers them automatically (watch-agents only reports them, and
hook confirmations stay owner-only), but an explicit answer is
now one flag away from approving a dangerous operation, with the
same ease as any ordinary menu.

# Design

- For the dangerous class (a permission prompt whose heading is
  a dangerous-operation warning), `agentctl answer` refuses
  unless the caller passes `--confirm-dangerous`.
- The refusal and a successful confirmed answer both name the
  matched warning line (the prompt's heading), so the record
  shows exactly what was approved.
- Hook confirmations stay refused outright, as today.
- Test in both editions where both exist (the Rust and Python
  TUI answer paths), with fixtures: dangerous form without the
  flag refuses and types nothing; with the flag it answers and
  names the warning; a non-dangerous permission form answers as
  before.

# Acceptance Criteria

- `agentctl answer NAME --option 1` on a dangerous-operation
  permission prompt refuses, names the warning line, and sends
  no key.
- `agentctl answer NAME --option 1 --confirm-dangerous` on the
  same form answers, verifies dismissal as today, and its
  report names the warning line.
- A non-dangerous permission prompt (for example a requires-
  confirmation rule) answers without the flag, unchanged.
- A hook confirmation still refuses even with the flag.
- Focused suites pass in both editions.

# Notes

[watermelon-full-preview] Filed for lane mu-musephantom by
coordb after #2045 agentctl-muse-phantom-background-send.
No lane, host or person is named here.
