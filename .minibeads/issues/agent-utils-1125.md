---
title: 'herdr-subagents-differentials: port the cross-language differentials, then flip the mirror to parity'
status: open
priority: 2
issue_type: task
assignee: mu-rsctl
created_at: 2026-11-09T10:44:50.000000000+00:00
updated_at: 2026-11-09T10:44:50.000000000+00:00
---

# Description

`validation/editions.json` records the `herdr-subagents` Rust mirror as `paused` (paused_at 456ed9ce, the landed B3-7a base): the Rust edition exists, is reachable only through the explicit `HERDR_SUBAGENTS_ENGINE` opt-in, and is not parity-tracked. That state exists because the same-name opt-in has no other legal home in the mirror-state model yet — `absent` cannot name the tracked `rs/bin/herdr-subagents` entrypoint, and `parity` requires a named cross-language differential job that does not exist today.

Unpause it as B3-9 of the herdr-subagents port: port the cross-language differentials for the `herdr-subagents` command surface (the sessions seam and the `agentctl migrate` surface land in the same slice of the campaign plan), then flip the mirror to `parity` naming the new job in `validation/cross.dag.yaml`.

# Design

- Add a `herdr-subagents` differential module under `cross/` in the shape of `cross/agentctl_differential.py`: paired Python/Rust invocations over scratch registries for every dispatched command and refusal, with the shared normalizers for timestamps, pids, and paths. Wire it as a gate-exclusive job in `validation/cross.dag.yaml` (the serial-admission pattern the other agentctl differentials use).
- Every target the harness starts is a process it spawned itself, addressed by recorded identity (pid plus start time, pidfd, or the turn's own cgroup) — the same signalling contract the B3 landing audits state.
- With the differential green in the full gate, flip `validation/editions.json`: mirror state `paused` -> `parity`, `parity_check` naming the new cross job, `paused_at` removed, `validation/editions.tsv` regenerated. Generic engine requests (`AGENT_UTILS_ENGINE=rust`) then honour the mirror; Python stays primary until the campaign flips that separately.

# Acceptance Criteria

- The differential job runs in the full gate and passes on the landing tip; it covers every `herdr-subagents` command the Rust edition accepts plus the refusals it still names (mcp until B3-8 lands).
- `python3 scripts/check_editions.py` passes with state `parity` and the regenerated TSV; the generic resolver refusal for the mirror is gone while the default stays Python.
- The editions note no longer describes the mirror as paused.

# Notes

Filed 2026-11-09 with the registration fix that put the mirror into `paused` (the B3-7b full gate caught the incomplete registration on d65b5c9c: absent-plus-entrypoint, an unlisted rs/bin entry, and a published-bin inventory mismatch). Related: #545 herdr-subagents-port (the port this unpauses), #1117 herdr-empty-stdout-actions (the sibling Herdr wrapper wart, fixed in both editions together).
