---
title: 'watch-standing-agents: standing agents may rest idle without a verdict and without idle notices'
status: open
priority: 2
issue_type: feature
assignee: cl-start
created_at: 2026-11-05T17:42:01.278507659+00:00
updated_at: 2026-11-05T17:42:01.278507659+00:00
---

# Description

Standing service agents (for example a consumer team's seven reviewers) rest idle between requests by design. `watch-agents` reports "idle without DONE:/BLOCKED:" for every one of them on every pass, which buries the real notices. Every idle knob is global (`--idle-minutes`, `--done-marker`, `--stall-minutes`, `--child-done-seconds`), and `agentctl pause` blocks delivery, so neither fits.

# Design

- A durable per-agent record flag, `agentctl set NAME --standing` / `--no-standing`, that survives restore and revive. A repeatable `watch-agents --standing NAME` may be added as well.
- For a standing agent, suppress ONLY the idle-without-verdict notice. Everything else stays: approval and permission prompts, guardrail refusals, a dead harness, stalls while working, queued input stuck behind a turn, and undelivered owner requests.
- `watch-agents --once`, and `agentctl requests` and status where natural, list standing agents separately, so the exemption stays visible.
- Rejected: pause, a bigger global `--idle-minutes`, and fake BLOCKED: lines.

# Acceptance Criteria

- Test: a standing agent idle past the idle threshold gets no idle notice. The same agent at an approval prompt, dead, or stalled mid-turn still gets those notices.
- Test: the flag survives restore and revive, and `--no-standing` restores the idle notice.
- `--once` output lists standing agents in their own section. Help and the userguide document the flag.

# Notes

Requested by the lead on 2026-11-05 at 04:27 AM ET (P2, from a consumer team's lead). The request was lost in the #884 pane-lock window: it sat in the coordinator's failed/ queue as possibly_submitted. It was recovered and filed at 12:50 PM ET. The lead deploys the landing SHA to the consumer team's watchers. Assigned to cl-start after #896 and #897.

## Progress (cl-start, 2026-11-06)

[opus 5.5] Implemented:
- `agentctl set NAME --standing` and `--no-standing`, a Rust-only command (the Python edition lists and refuses it, as it does `manage`). The flag is a `standing: true` key in the agent's record, in the flat record or a nested row's extensions. Revive copies it to the new generation, and restore revives, so it survives both. Rename keeps the record.
- `watch-agents --standing NAME`, repeatable, exempts an agent for one watcher only.
- For a standing agent, `watch-agents` drops only the `idle-no-verdict` finding, for screen, generic and headless agents alike. Approval and permission prompts, refusals, dead, stalled, turn-queued, child-done, held queues and owner waits are unchanged.
- Each pass's report gains a `standing` list. `list` and `status` rows show `standing: true` for such an agent.
- Docs: the help for `set` and `--standing`, and the agent userguide's watch-agents section.

Tests:
- A standing agent idle past the threshold gets no episode or notice and is listed under `standing`. The same agent at an approval prompt, dead, or stalled while working is still reported.
- Without the flag the idle notice is back, and `--standing NAME` exempts it.
- The flag survives revive, shows in status, and `--no-standing` clears it.

Not done: `agentctl requests` does not list standing agents separately; its rows are requests, not agents.
