---
title: 'muse-launch-overrides-input: agentctl refuses to type into a Muse launched with launch-flag overrides, and reports the refusal as an ambiguous outcome'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-10-20T12:23:37.166153286+00:00
updated_at: 2026-10-20T12:23:37.166153286+00:00
---

# Description

[opus 5.5] Since #382 muse-approval-never-ready, a Muse launched from a profile whose argv overrides saved settings (here --trust-workspace --approval-mode never; footer 'Launch overrides') is recognised as an idle composer, but agentctl start and send still refuse the first prompt: run/run_with_runtime in rs/agentctl/src/subagents.rs and prompt_agent in py/agentctl/subagents.py accept input only when the footer reads Auto-review. That refusal happens before any input, yet it is raised as an ordinary unavailable error, which the delivery path treats as an unknown outcome after an injection: the message is quarantined under queue/failed as possibly submitted (exit 76) although the pane shows an empty editor. Every Muse launch from the owner's profile has needed a hand-pasted brief. The refusal text cites a missing Herdr 'effect-coupled process-generation guard', i.e. a write that Herdr refuses atomically unless the pinned foreground process is still the one checked. That would close the window between agentctl's last process check and Herdr's write, but the window is the same for every footer mode, and a program that takes the pane in that window is not the one whose footer was read, so the footer mode does not bound a misroute.

# Acceptance Criteria

Captured-pane fixtures for the Auto-review and Launch overrides footers. An owned Muse session delivers at any single verified footer mode with the existing process, staged-editor and transcript proofs; an adopted pane keeps the Auto-review requirement. Every refusal raised before any input is not-staged in both editions: the message stays pending (exit 75), never possibly submitted. Both editions agree; differential coverage stays green; one end-to-end start with the muse-watermelon profile delivers its brief without a hand paste.
