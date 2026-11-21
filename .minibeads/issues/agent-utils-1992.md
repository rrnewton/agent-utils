---
title: 'agentctl-revive-journal-launch-spec: finish an in-flight revive against its recorded launch spec and pin-abandon a vanished pane'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- revive
created_at: 2026-11-20T12:09:00-05:00
updated_at: 2026-11-20T12:09:00-05:00
---

# Description

[watermelon-full-preview] From a consumer report (2026-11-20): a
boot revive launched the new copy of a lane (alive) but never
finished. Every command on the name now returns rc 75, "revive ...
is incomplete; rerun ... --expected-token", and the rerun can never
complete. The dry run says "action: blocked, resumed candidate has
no independently pinned runtime", even though the candidate's
harness is alive in its pane. Two suspected causes, to reproduce
before fixing:

- The newer agentctl rebuilds the expected launch from the
  profile's current definition. A profile that now adds arguments
  the build which started the revive did not pass (a profile-gained
  settings flag) makes the recorded candidate's arguments never
  match, so candidate verification can never pass.
- Finishing needs the old pane, and it is gone.

Reproduce in a scratch registry first; never touch the reporting
consumer's registry.

# Design

- (a) An in-flight revive journal completes against its own
  recorded launch spec; only a new revive re-reads the profile.
  The candidate's expected arguments, adapter and policy are
  checked against what the journal's generation recorded, not
  against today's profile definition.
- (b) A vanished old pane is finishable, or abandonable via an
  explicit pinned verb (name plus expected candidate token plus
  record digest), without retire-stale-registration followed by
  adopt. The proofs stay the same: the candidate's harness lives
  in its pane on the recorded session (the #1934 publish path),
  and abandon requires that no harness runs for the candidate.

# Acceptance Criteria

- A scratch-registry reproduction of the stuck journal (profile
  gained an argument mid-revive; old pane vanished) finishes with
  `agentctl revive NAME` and publishes the live candidate.
- The same journal with a dead or mismatched candidate is still
  refused, and `--abandon-candidate` still refuses while a harness
  runs for it.
- A pinned abandon for the vanished-pane case succeeds only with
  the exact token and record digest, and refuses otherwise.
- Tests cover the profile-drift finish and the pinned abandon;
  focused revive suites stay green.

# Notes

[watermelon-full-preview] Lane mu-restorefix/1992, after #1934
agentctl-restore-muse-sessions-and-timeout landed (c8591bd3),
coordinated by coordb. Related: #1934, #1928.
