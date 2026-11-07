---
title: 'agentctl-restart-profile-refresh: restart leaves a Muse lane dead, and revive ignores the current profile'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-07T21:40:00.000000000+00:00
updated_at: 2026-11-07T21:40:00.000000000+00:00
---

# Description

On 2026-11-07 an environment variable was added to a Muse profile (`env: {MUSE_EXPERIMENTAL_ULTRA_REASONING_EFFORT: on}`, which makes the requested `ultra` reasoning effort take effect). Moving a running lane onto it failed in two ways:

- `agentctl restart NAME --when-idle` stopped the idle Muse lane, which printed "To continue this session, run muse resume <id>" and exited to a shell prompt. Nothing relaunched it; the record's status read `unknown`.
- `agentctl revive NAME` then relaunched it from the recorded launch spec. The new process had no profile env, so it still ran at the gated-off level ("reasoning effort ultra is not available ... using xhigh").

What worked: `agentctl stop NAME`, then `agentctl start NAME --profile P --resume <native session id>`. That costs a new record generation and a slot re-bind, and the operator has to find the session id by hand.

# Design

- `restart` relaunches the harness it stopped, in the same pane, and reports a failure rather than leaving a shell prompt.
- `restart` and `revive` gain `--profile P` (or `--refresh-profile`), which rebuilds the launch spec from the named profile (argv, env, model, reasoning effort) while keeping the recorded native session, queue and token.
- `status` shows when a record's launch spec differs from its profile's current definition, so an operator can see which lanes still run an old profile.

# Acceptance Criteria

- A test: `restart` on an idle Muse stand-in leaves a running harness in the pane, or exits non-zero with the reason.
- A test: after a profile's env changes, `revive NAME --refresh-profile` (or `restart --profile`) starts the harness with the new env and the same session.
- A test: `status` names a launch-spec/profile mismatch.

# Notes

Filed by the devbig030 coordinator at the lead's request. Both teams moved their Muse lanes with the stop plus `start --resume` workaround.
