---
title: 'headless-approval-surface: route a headless agent''s permission prompts to the owner through the chat bridge, and show full denial reasons in tail'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-10-28T19:20:14.306897689+00:00
updated_at: 2026-10-28T19:20:14.306897689+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 15:10 ET: guardrails are hobbling another harness's headless coordinator ("It basically quickly became useless after switching it to headless"), and "our console tailing just prints this: denied bash. The agent told me that it had more context, like a reason for the refusal. If we have that, we should print that also." Evidence (read-only, that coordinator's event log): 282 tool calls, 12 denied: 8 by its own PreToolUse permission hook, 4 'requires approval, and this session has no approval surface ... denied automatically' (the wrapper passes --permission-prompts none). Build: (1) tail prints a refused call's full reason (wrapped, up to ~600 characters) with its tool and command, and the event log keeps the full masked refusal text, not a one-line preview; (2) an approval surface: the wrapper runs Claude Code with --permission-prompts host --permission-prompt-tool stdio, answers each can_use_tool prompt itself, logs approval.requested, and the chat bridge posts 'AGENT wants to run: ... (reason: ...) - reply approve or deny' to the channel; the owner's verified chat reply (or the owner at a terminal, #670 agentctl-console) decides; no answer within the timeout (default 10 min, configurable) denies; never auto-approved; every decision logged as approval.decided. Hook hard refusals stay refusals (they never prompt). (3) Tests with a fake harness and a fake bridge: approve, deny, timeout, tail rendering. Related: #665 bridge-stuck-agent-alerts (pane agents; headless turns those dialogs into this path), #670 agentctl-console.
