---
title: 'revive-candidate-auto-relaunch: a candidate whose harness exits immediately strands in no-pinned-runtime limbo'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T13:53:37.000000000+00:00
updated_at: 2026-11-09T13:53:37.000000000+00:00
---

# Description

[watermelon-full-preview] From the 2026-11-09 host reboot recovery. A
revive candidate whose harness exits immediately can be stranded with
"resumed candidate has no independently pinned runtime"
(verify_candidate, rs/agentctl/src/subagents/revive.rs): the candidate can
neither be verified nor abandoned, and every later revive or restore hits
the same wall. Recovering one lane by hand needed a manual tab close, then
retire-stale-registration, then revive --archived - retiring the whole old
generation to get around a candidate that abandon_revive already knows how
to drop safely.

The dead ends are classification gaps in the #899/#920 abandon machinery.
check_candidate calls a candidate Live when Herdr reports any agent in its
pane, without comparing kind or session to the candidate, so a foreign or
wrong-session harness makes an unanchored candidate unabandonable.
failed_candidate_pane requires lifecycle launch_failed, so a candidate
still marked starting (the revive process died mid-launch, as at boot)
never auto-closes. A candidate in a reused old pane is excluded from the
auto-close arm entirely. And the error text advises --abandon-candidate,
which itself refuses for Live candidates and non-launching phases.

# Design

Inside revive_once's existing candidate match, so CLI revive and restore
both gain it with no API change:

- Widen the failed-candidate-pane arm from launch_failed to
  starting|launch_failed, keeping the own-pane proof (tab and workspace
  census) before any pane is closed.
- Add a not-ours classification: an unanchored candidate (no terminal and
  no process identity - exactly verify_candidate's failing half) whose
  pane reports a different harness kind or a different native session is
  abandoned without closing the pane (it holds a foreign harness), with
  the mismatch recorded as evidence. A pane reporting the candidate's own
  session is never abandoned.
- Boundedness stays as today: at most one abandon plus one relaunch per
  revive invocation; a relaunch failing the same way returns an error with
  a fresh journal for the next invocation.

# Acceptance Criteria

- A starting, unanchored candidate in its own non-idle pane is auto-closed,
  abandoned, and relaunched by a plain revive, with no --abandon-candidate.
- A pane reporting a different session: candidate abandoned, pane left
  running, lane relaunched in a new tab, evidence names the mismatch.
- A pane reporting the candidate's own session, and identity-anchored
  candidates, behave exactly as today (never auto-abandoned).
- One restore run brings a lane blocked this way back and the row carries
  the abandoned-candidate evidence.

# Notes

Manual recovery (tab close, retire-stale-registration, revive --archived)
stays available and unchanged; this issue only removes the cases that
forced it. Related: #899/#920 candidate abandon paths, #1137
restore-waits-for-certificate (its died-after-launch rows feed the same
relaunch-on-rerun expectation).
