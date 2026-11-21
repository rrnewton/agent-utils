---
title: 'agentctl-claude-permission-answer: answer sees no question on a permission prompt prompts classifies as menu_or_picker'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-20T18:36:00-05:00
updated_at: 2026-11-20T18:36:00-05:00
---

# Description

[watermelon-full-preview] `agentctl answer NAME --option N` reports
"no supported blocking question is visible" for a Claude Code
permission prompt ("Dangerous rm operation ... Do you want to
proceed? 1. Yes 2. No") that `agentctl prompts` classifies as
menu_or_picker. The prompt is visible and classified, but the answer
path has no recogniser for this permission form, so an operator
cannot answer it through agentctl and must type into the pane.

# Design

- Add a recogniser for this Claude Code permission form to the
  answer path, matching the exact form `prompts` already classifies,
  so `--option N` selects the numbered choice it names.
- Pin it with a fixture from a real capture of the form, with any
  real paths in the captured command replaced by neutral ones.
- Keep the never-retry-uncertain rule: an answer whose dismissal
  cannot be verified is reported, not retried.

# Acceptance Criteria

- `agentctl answer NAME --option 2` on the captured permission form
  selects "No" and verifies the prompt dismissed; `--option 1`
  selects "Yes".
- A fixture test pins the recogniser against the captured form, and
  unlike forms (a menu_or_picker of another shape) still report no
  supported question.
- The fix is exercised against a test agent through the real CLI
  before landing.

# Notes

[watermelon-full-preview] Filed for lane mu-musedeliver, queued
after #1995 agentctl-muse-working-delivery (coordinated by
coordb). No lane, host or person is named here.
