---
title: 'harness-crate-consolidation: move every per-harness fact into the agent-harness crate'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
depends_on:
  agent-utils-710: related
created_at: 2026-10-30T21:05:01.156637263+00:00
updated_at: 2026-10-30T21:05:01.156637263+00:00
---

# Description

[opus 5.5] Follow-up of #710 chat-transcript-extraction (report section 7). #710 starts rs/agent-harness with transcript location and decoding for Claude Code, Codex and Muse. Move the rest behind it: agentctl's harness adapters and transcript Activity readers, harness_tui forms, screen markers and composer rules, the chat reply scanner's harness rules, launch and resume arguments, follow-up policy; retire the private transcript readers in chat_service/reply_reminders.rs and inbox/watch.rs; point wrkviz's Rust edition (#706 rust-primary-wrkviz) at the crate's event model, with the Python importers as the differential oracle. Decide whether its agent-transcript CLI becomes a suite tool (RELATED_WORK comparison is in the report).
