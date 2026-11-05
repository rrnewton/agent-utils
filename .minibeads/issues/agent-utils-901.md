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
