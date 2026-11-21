---
title: 'agentctl-adopt-attach-profile: attach a launch profile at adopt or by pinned verb so restore and revive use it'
status: closed
priority: 2
issue_type: task
labels:
- agentctl
- adopt
- profiles
created_at: 2026-11-21T03:30:00-05:00
updated_at: 2026-11-21T03:30:00-05:00
---

# Description

[watermelon-full-preview] `agentctl adopt` freezes the running
process's command line as the launch recipe, with no profile.
Restoring such a record replays it without the profile's settings
(lane hooks and permissions) and without the profile's env (the
git author and committer identity). The operator wants the lane
on its profile after the fact.

No new persisted record field: the record's existing `profile`
field is what gets set (confirmed with the coordinator before
starting).

# Design

- `agentctl adopt --profile P` names the launch profile at
  adoption. The profile must exist and its harness kind must
  match the adopted pane's harness; anything else is refused.
- A pinned verb, `agentctl bind-profile NAME --profile P
  --expected-token T`, attaches a profile to an existing adopted
  record. Same existence and harness-kind checks.
- After attaching, restore and revive rebuild the launch from
  the profile as they already do for profile-started lanes, and
  `agentctl status` shows the attached profile.
- Owner rule 257 stands: restore only reports records without a
  profile and never rewrites them; attaching is an explicit
  operator action.

# Acceptance Criteria

- `adopt --profile P` records P on a pane whose harness matches
  P's harness, and refuses a missing or mismatched profile.
- `bind-profile NAME --profile P --expected-token T` attaches P
  to the existing record only with the exact token, refusing a
  wrong token, a missing profile, or a harness mismatch.
- Revive of a bound record rebuilds from P's argv and env, and
  status shows the profile.
- Tests cover both verbs and the refusals; focused adopt/revive
  suites stay green.

# Notes

[watermelon-full-preview] Lane mu-restorefix/1993, after #1992,
coordinated by coordb. Related: #1934, #1928, owner rule 257.

## Resolution

[watermelon-full-preview] Landed as c18e80b6 on main. `adopt
--profile P` and `bind-profile NAME --profile P --expected-token T`
attach a launch profile in both editions: the profile must exist
and name the record's harness kind and mode, the bind is pinned to
the record's exact token, and no new record field was written (the
existing `profile` field is set). Afterwards revive and restore
rebuild the launch from the profile and status shows it. Verified
on the landing tree: fmt and clippy clean, Rust
adopt/bind/revive/restore suites 304 passed, Python
herdr/identity/lifecycle/restore-drill suites 415 passed.
