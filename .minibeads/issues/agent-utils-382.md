---
title: 'muse-approval-never-ready: a Muse launched with --approval-mode never never reaches a verified idle composer'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-10-19T21:53:26.643339249+00:00
updated_at: 2026-10-19T22:29:16.677717147+00:00
closed_at: 2026-10-19T22:29:16.677717067+00:00
---

# Description

[muse watermelon] A Muse started through agentctl with the extra harness argument --approval-mode never is never recognised as having reached a verified idle composer, so agentctl start reports launch_failed with: custom harness muse did not reach a verified idle composer. The harness itself runs fine. Root cause found by reading a live pane: with launch-flag overrides the Muse status footer fourth field reads Launch overrides instead of YOLO or Auto-review, and muse_status_mode in rs/agentctl/src/client/muse_editor.rs (and its line-for-line port in py/agentctl/client.py) accepts only YOLO, Auto-review, or a three-field standard footer, so muse_composer_regions returns none and muse_idle_composer is never true. Fix the probe in both editions with a test on a captured pane fixture.

# Acceptance Criteria

A captured Launch overrides footer frame reads as an idle composer in both editions; existing ambiguous-footer and choice-prompt refusals still refuse; differential coverage stays green.
