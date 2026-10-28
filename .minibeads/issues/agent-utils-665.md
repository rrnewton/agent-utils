---
title: 'bridge-stuck-agent-alerts: the chat bridge detects a stuck agent and warns in the channel'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-10-28T17:39:22.172637896+00:00
updated_at: 2026-10-28T17:39:22.172637896+00:00
---

# Description

[opus 5.5] owner 12:16 ET. Another harness's coordinator froze on a Claude Code approval dialog raised by its native general-purpose subagent ("This shell -c script runs rm and could not be checked / Do you want to proceed? 1. Yes 2. No"), stalling the whole chat experience. Build: (1) the chat bridge decides its bridged agent is UNHEALTHY from combined signals: no tool output or turn progress for N minutes while a request is pending, plus pattern matches for known blocking prompts (Claude permission dialogs, bash-safety "could not be checked", Codex cyber/safety filters, Muse approvals), kept in a per-harness pattern table in config (no scattered TUI hacks); (2) it posts a deterministic WARNING into the chat channel with a short screen sample; (3) for matched patterns it OFFERS a deny action the owner can trigger from chat (never auto-answers yes); (4) same signals feed watch-agents for subagents. Also: agentctl skill/guide text: a coordinator with an active chat bridge must not do substantial work in its own session or native subagents (they can block the coordinator on approval dialogs); delegate to slot lanes. Note headless mode turns these into refusals the agent can recover from.
