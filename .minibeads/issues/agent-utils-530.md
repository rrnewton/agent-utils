---
title: 'slack-outbound-format: selectable converted Slack outbound path preserving Markdown fidelity'
status: closed
priority: 2
issue_type: feature
created_at: 2026-10-26T14:26:01.776575683+00:00
updated_at: 2026-10-26T18:46:21.101952295+00:00
closed_at: 2026-10-26T18:46:21.101952155+00:00
---

# Description

[muse watermelon] Owner 2026-10-26: keep both Slack outbound paths active and switchable until a clear winner. Current path hands raw Markdown to the vendor CLI as --markdown and its server conversion drops blank lines, glues nested-bullet tables, turns headings into header blocks. Build a second path: agentctl converts Markdown itself (comrak) and posts with --slack-mrkdwn (or Block Kit where it round-trips), preserving paragraphs, formatting, lists, headings as bold lines, quotes, code fences, tables as aligned monospace blocks, task boxes. Escape per Slack, cap 4000 bytes per part split at block boundaries. Switch via chat config (per bridge/channel), env/flag override, default markdown; chat status shows active; log path per post content-free. Comparison harness with scrubbed corpus, dry-run for CI, live posting only on explicit coordinator OK to a test channel. Part 0 (coordinator add): fix chat reply capture joining terminal soft-wrapped rows at pane width (never inside code fences), with screen fixtures; check current build.

# Acceptance Criteria

Converted converter + tests pass; --format/--slack-format/AGENTCTL_SLACK_FORMAT switch works with default markdown; chat status shows active format; per-post content-free log; harness dry-run scores corpus in ai_docs/transient/2026-10-26-slack-outbound-format/; soft-wrap capture fix + fixtures; chat userguide documents both paths; validate gate green.
