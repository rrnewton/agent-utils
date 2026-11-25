---
title: 'agentctl-watch-non-agent-registry-dirs: a registry directory without agent.json, such as hooks/, is never read as an agent'
status: in_progress
priority: 1
issue_type: bug
labels:
- agentctl
assignee: cl-hookdirs
created_at: 2026-11-25T09:20:00-05:00
updated_at: 2026-11-25T09:20:00-05:00
---

# Description

A consumer's `agentctl watch` reports a problem it cannot repair every pass: "hooks_trust hooks: cannot read JSON REGISTRY/hooks/agent.json: No such file or directory". The registry's `hooks/` directory holds only a permission hook script; it never held an agent record. The watch read that directory as an agent named "hooks" and asked the Muse hooks-trust step (#1932) to answer for it.

A second notice, "hooks_trust NAME: unknown agent", appeared right after an agent was stopped: the hooks-trust sweep listed the agent, the stop archived its record while the sweep waited for the agent's lifecycle lock, and the sweep's re-read of the record failed.

Root cause. The first notice comes from a watch built before #2121, whose registry name listing took every top-level directory with an agent-shaped name (layout 1) as an agent; #2121 made the listing require an `agent.json` in either layout. The second is a check-then-act race in `answer_hooks_trust_if_dialog`: the record is checked once, then loaded before and after taking the lifecycle lock, and a stop in between turns the load into an "unknown agent" error.

# Design

- A directory holding no `agent.json` in either layout is never an agent: not listed by `list`, the watchers or the hooks-trust sweep, and never a watch problem. Only an existing record that cannot be read is reported.
- The hooks-trust answer treats a record that disappears at any point before the keypress (a stop or archive racing the sweep) as nothing to answer: no keypress, no finding. An existing malformed record still fails.
- The watch-agents hooks step skips an agent that is no longer registered when the step runs, so an observation taken earlier in the pass never leads to an attempt on a removed agent.
- `doctor` reports a recordless top-level (layout 1) directory as record-absent only when it holds a queue; a directory of the operator's own files, such as `hooks/`, is not an agent directory.
- No persisted format changes.

# Acceptance Criteria

- Hermetic tests: a registry with a `hooks/` directory holding a script, in either layout, lists no "hooks" agent, and a watch pass reports no finding for it.
- Hermetic test: an agent stopped and removed while the hooks-trust step waits for its lock gets no keypress and no finding.
- Hermetic test: the watch-agents pass makes no hooks-trust attempt for an agent that is no longer registered.
- No wall-clock budgets in the tests; cargo fmt and clippy clean.

# Notes

Reported by a consumer's lead; follows #2121 and #1932.
