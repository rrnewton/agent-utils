---
title: 'agentctl-status-redact-argv: redact launch arguments in status and other human-facing output'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- security
created_at: 2026-11-20T02:09:21-05:00
updated_at: 2026-11-20T02:09:21-05:00
---

# Description

[watermelon-full-preview] `agentctl status NAME` prints the agent's launch arguments (model, effort, permission flags, and potentially secrets) because the status row embeds the durable record, whose `launch_spec.argv` is the exact argument vector after the program. Launch argv is exactly where a token can leak; a token leaked through argv in an incident on 2026-11-14 (recorded by the requesting lead; no consumer is named here). Anyone who can read status output, a revive dry-run plan, a restore row, or a log line that echoes the spec can recover values that were never meant to be displayed. Environment values must never be printed either; the spec already stores environment names only, and that invariant must hold in every rendered form.

Goal: human-facing output shows that a launch happened and which flags were used, without showing their values, unless the operator explicitly asks for the full vector.

# Design

- Redact `launch_spec.argv` in `status` (and in any other human-facing output that prints `launch_spec.argv`, such as revive dry-run plans, restore rows, and logs): show flag NAMES only with values elided, e.g. `--model <set>`. Positional values and `flag=value` values are elided the same way; the program name itself is not secret and may stay.
- Full argv only behind an explicit flag such as `--show-argv` on the commands that render a spec. Internal replay, validation, and the persisted record are unchanged: revive still replays the exact recorded vector, and no persisted field changes (reader-skew check: this issue adds no persisted field; if implementation finds one is needed, report it before gating).
- Never print environment values anywhere in these outputs; environment names only, as today.
- Both editions (Python `py/agentctl` and Rust `rs/agentctl`) behave identically, including the flag name and the elided rendering, so parity tests can compare them.
- Tests drive the exact defect: a token-like value placed in argv never appears in status output (nor in the other covered renderings), with and without the explicit flag.

# Acceptance Criteria

- `agentctl status NAME` output contains no argv value bytes: flag names appear with values elided (e.g. `--model <set>`), and a token-like argv value never appears, in both editions.
- Revive dry-run, restore rows, and any log/output path that prints `launch_spec.argv` apply the same redaction; no environment values appear in any of them.
- Full argv is available only behind the explicit `--show-argv` flag (or the agreed equivalent), off by default, in both editions.
- The persisted record and revive replay are byte-for-byte unchanged; existing launch-spec, revive, and parity tests stay green.
- New tests place a token-like value in argv and assert it never appears in the covered outputs; they pass on a host above 75% CPU with condition-based waits only (owner rule 346).

# Notes

Security hygiene, requested 2026-11-19 by a consumer's lead via the coordinator; the consumer is not named (repository boundary rule). Lane mu-agentctlfix, nested worktree `.muse/worktrees/agentctlfix` on branch `mu-agentctlfix/1936`. Related: #813 agent-record-schema-forward-compat (spec keys are preserved; redaction is render-only). Companion: #1938 agentctl-pidfd-open-einval, same lane, separate commit.
