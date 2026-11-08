---
title: 'rust-wrkviz-agent-scope-port: port --agent-scope coordinator and summarize-project to Rust before the A3 flip'
status: open
priority: 1
issue_type: task
created_at: 2026-11-08T17:18:34.764426174+00:00
updated_at: 2026-11-08T17:18:34.764426174+00:00
---

# Description

[opus 5.5] Two Python-only wrkviz surfaces landed after the Rust-primary inventory: summarize --agent-scope coordinator (a coordinator-only summary tree under teams/<slug>/summary_scopes/coordinator/, with the kept selection recorded and claude-history and missing-root refusals) and the summarize-project subcommand (a project weekly from team weekly technical rollups under summary_scopes/<scope>/project/<name>/). The Rust edition prints the same help and usage but refuses both by name with exit 2. The A3 default flip (planned 2026-11-13) would break a recurring coordinator-only weekly run unless both are ported first or that run pins WRKVIZ_ENGINE=python.

# Acceptance Criteria

The Rust edition implements --agent-scope coordinator and summarize-project at differential parity with Python (tree, receipts, summarize_state, Markdown, refusals), or the A3 flip is held, or the weekly run pins WRKVIZ_ENGINE=python with that recorded here; ai_docs/transient/2026-11-07-rust-primary/agentctl-wrkviz-schemas.md item 2 is updated either way.
