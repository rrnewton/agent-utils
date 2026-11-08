---
title: 'agentctl-restart-profile-refresh: restart leaves a Muse lane dead, and revive ignores the current profile'
status: closed
priority: 3
issue_type: bug
assignee: cl-sync
created_at: 2026-11-07T21:40:00.000000000+00:00
updated_at: 2026-11-08T05:30:59.000000000+00:00
closed_at: 2026-11-08T05:30:59.000000000+00:00
---

# Description

On 2026-11-07 an environment variable was added to a Muse profile (`env: {MUSE_EXPERIMENTAL_ULTRA_REASONING_EFFORT: on}`, which makes the requested `ultra` reasoning effort take effect). Moving a running lane onto it failed in two ways:

- `agentctl restart NAME --when-idle` stopped the idle Muse lane, which printed "To continue this session, run muse resume <id>" and exited to a shell prompt. Nothing relaunched it; the record's status read `unknown`.
- `agentctl revive NAME` then relaunched it from the recorded launch spec. The new process had no profile env, so it still ran at the gated-off level ("reasoning effort ultra is not available ... using xhigh").

What worked: `agentctl stop NAME`, then `agentctl start NAME --profile P --resume <native session id>`. That costs a new record generation and a slot re-bind, and the operator has to find the session id by hand.

# Design

- Cause of the dead lane: restart's exit step waited only until the pane showed an idle shell. Muse prints its resume hint and returns to the shell while its runtime is still ending, so the revive inside restart found the pinned harness process alive (or its death unprovable) and refused; the restart journal stayed at `exited`. A later revive succeeded because the runtime had gone by then.
- `restart` now waits, after the exit, until the pinned harness process is provably dead (10 s grace), then ends a leftover process tree as a resume restart does (#920), and refuses with the reason if death still cannot be proved. Any relaunch failure after the exit says the harness exited, that the pane holds a shell, and that `agentctl revive NAME` finishes the restart.
- A restart journal naming the same agent under an earlier generation's token can never resume; restart removes it (after its preflight, so a refused restart changes nothing).
- `revive NAME --refresh-profile` and `restart NAME --refresh-profile` rebuild the stopped record's launch policy from its profile's current definition (model, reasoning effort, environment names, launch-spec argv) and keep its native session, queue and token. `restart` preflights the refreshed launch before it exits anything. Refused for a record with no profile, a profile that now names another harness or mode, or (revive) a harness that still runs. The Rust edition only: the Python edition refuses the flag.
- `agentctl status NAME` shows `profile_drift` (the fields that differ and the fix command) when a record's launch policy differs from its profile's current definition. Only single-agent status checks it, since reading profiles can run git.

# Acceptance Criteria

- A test: `restart` of a Muse stand-in whose runtime outlives its exit ends the leftover and leaves a running harness; one whose relaunch fails exits non-zero naming the exited harness and `agentctl revive NAME`.
- A test: after a profile's env and argv change, `revive NAME --refresh-profile` and `restart NAME --refresh-profile` start the harness with the new env on the same session.
- A test: `status` names a launch-spec/profile mismatch, and the refresh is refused while the harness runs or when the profile changed harness.
- A test: a stale restart journal from an earlier generation is removed by the next restart.

# Notes

Filed by the devbig030 coordinator at the lead's request. Both teams moved their Muse lanes with the stop plus `start --resume` workaround.

## Resolution

[opus 5.5 cl-sync] Fixed in the Rust edition as the Design above says, with tests in
`rs/agentctl/src/subagents/tests/revive.rs` for each criterion. The dead-lane cause is inferred
from the code and the leftover `exited` journal (the original restart's stderr was not kept); the
lingering-runtime path is covered by a fake, not yet by a live Muse restart.
