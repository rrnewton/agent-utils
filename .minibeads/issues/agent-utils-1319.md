---
title: 'codex-effort-ultra: agentctl refuses max and ultra effort for Codex lanes that Codex itself accepts'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-11T16:27:03+00:00
updated_at: 2026-11-11T16:27:03+00:00
---

# Description

[opus 5.5] #1216 agentctl-effort-option added a per-harness table of accepted reasoning efforts (`effort_levels` in rs/agentctl/src/profiles.rs). It lists Codex as minimal, low, medium, high, xhigh. The Codex release in use declares max and ultra for current models, and Codex lanes have been running for days with `--config model_reasoning_effort=ultra`.

Since #1216, a build of agentctl refuses to start any lane whose profile sets Codex effort to ultra:

    reasoning effort "ultra" is not supported for harness "codex"; accepted levels: minimal, low, medium, high, xhigh

Builds from before #1216 start the same profile without complaint. So once the daily services turnover moves a host to a post-#1216 build, every start, revive, restore or relaunch of a Codex ultra lane fails. Running services are not affected.

# Design

- Codex accepts minimal, low, medium, high, xhigh, max, ultra. Codex validates the level against the chosen model itself, so agentctl should not narrow the list further.
- Check that the Python edition (py/agentctl) agrees if it keeps its own level table.
- Check that the Claude and Muse tables match their current CLIs. Muse ultra needs its gate environment variable; keep ultra in the Muse list.

# Acceptance Criteria

- `agentctl start --profile P` with a Codex profile whose reasoning_effort is ultra (or max) passes `--config model_reasoning_effort=ultra` (or max) and is not refused.
- Tests in rs/agentctl/src/subagents/tests/effort.rs cover Codex max and ultra, and still refuse a level no harness knows.
- The Rust and Python editions agree on every harness's levels.

# Notes

Reported by a consumer's coordinator, whose lander could not start a Codex lane on the newer services snapshot. P1: the next scheduled turnover would otherwise break every Codex ultra relaunch on that host.
