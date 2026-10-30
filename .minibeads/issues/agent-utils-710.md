---
title: 'chat-transcript-extraction: read agent replies from the harness transcript, not the screen'
status: closed
priority: 1
issue_type: feature
labels:
- agentctl
- wrkviz
created_at: 2026-10-30T20:38:09.342031375+00:00
updated_at: 2026-10-30T22:04:23.345316796+00:00
closed_at: 2026-10-30T22:04:23.345316646+00:00
---

# Description

[opus 5.5] Owner 2026-10-30 16:36 ET: the chat bridge scrapes the TUI for agent replies. Replies rendered on screen lose their text (tables become box drawing, long lines wrap, mid-turn text collapses to a one-line summary) and answers given only in the TUI never reach chat at all.

Read the running agent's own transcript instead (Claude Code session JSONL, Codex session rollouts, Muse session store), through one shared transcript-reading library used by both wrkviz and agentctl.

Plan:
1. Report first (private harness repo): principled transcript access per harness, evidence from replaying real coordinator transcripts against what was posted, and an incremental plan.
2. (a) clean text source: keep today's reply fences, but take each reply's text from the transcript; the screen stays as fallback and as the state source.
3. (b) later, if approved: a fence-less, transcript-primary mode that slices the agent's natural output into chat messages.

Subsumes #662 chat-capture-from-transcript.
