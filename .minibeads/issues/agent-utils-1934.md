---
title: 'agentctl-restore-muse-sessions-and-timeout: record Muse sessions at start, make restore resume, publish live revive candidates'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
- restore
- muse
created_at: 2026-11-19T22:35:00-05:00
updated_at: 2026-11-19T22:35:00-05:00
---

# Description

[watermelon-full-preview] Two teams' boot restores failed on 2026-11-19
(host reboot at about 10:35 PM ET). Three defects combined:

- Both registries' boot restores (`agentctl restore --boot` under a unit
  with `TimeoutStartSec=1800`) timed out at 30 minutes, were restarted,
  and left no completed report for the boot. One team has about 70 lanes
  at about a minute each, so a single bounded attempt cannot finish it,
  and the restarted unit started over instead of resuming.
- `agentctl revive NAME` refused every Muse lane started with
  `agentctl start`: "record has no native conversation ID: 265 muse
  sessions in the session store name its cwd, and agentctl does not
  guess". The start path never recorded the Muse session id, so after
  the reboot revive could only scan the store by cwd and refuse.
- Half-finished revives left records blocked ("resumed candidate has no
  independently pinned runtime"), and `--abandon-candidate` is refused
  while the candidate's Muse lives. Manual recovery worked through
  retire-stale-registration (`--expected-revive-sha256`,
  `--replacement-pane`, `--replacement-session`) followed by adopt.

Revived Muse lanes also stopped at the "Review plugin hooks" trust
dialog; #1932 agentctl-hooks-trust (lane mu-hookstrust) is fixing that
part. This issue coordinates with it and does not duplicate it.

Reader skew: a consumer runs older agentctl builds. Any new persisted
record field must be reported before gating, and old binaries must
tolerate it. The fix below reuses the existing `native_session` record
field, so no new persisted record field is introduced.

# Design

- (1) Record the Muse native session id at start and promote: observe
  it from the launched pane's own Muse process (the session directory
  its runtime holds open under the Muse session store, or the store's
  own per-pid session report), pinned to that process's identity, and
  store it as `native_session` exactly as other harnesses already do.
  Muse's foreground runtime is recognised as a native group leader so
  the pane pin survives its helper processes, and any held path inside
  the session directory (the directory itself, its journal, or its
  lock) names the session. Backfill: a doctor repair records the
  session for live Muse (and Codex) lanes whose records lack it,
  using the same pinned observation; several held sessions are never
  guessed between.
- (2) Restore makes progress within any unit timeout: it writes a
  partial report as it goes (after the coordinator and after each
  lane, never marked complete), and on restart in the same boot epoch
  it resumes where it left off, carrying forward lanes already revived
  this boot after re-proving them alive instead of reviving them
  again. The shipped units keep `TimeoutStartSec=1800`, now justified
  as a per-attempt progress watchdog rather than a whole-team budget:
  each attempt makes durable progress and the bounded restart resumes
  it, so the timeout no longer needs to cover every lane at once.
- (3) The half-finished revive case: when a retained candidate's
  harness is alive in its pane with a session that matches the record,
  revive completes by pinning and publishing the candidate (the
  automated form of retire-stale-registration plus adopt, with the
  same proofs: pane, tab and workspace, harness kind, live pinned
  process, matching session, no other claimant). It refuses otherwise.
- (4) Tests drive a fake Muse in a fake pane exposing a session id:
  start records it, revive after a simulated reboot resumes it,
  restore restarted midway finishes, and a half-finished candidate is
  published. Condition-based waits only (owner rule 346).
- (5) `common/docs/agentctl/harness-support-matrix.md` and the
  team-restore skill record the new behaviour in the same change.

# Acceptance Criteria

- A Muse lane started with `agentctl start` (and one repaired by
  `agentctl promote` after a launch failure) carries its observed
  `native_session`, and revive/restore resume that session after a
  simulated reboot instead of scanning the store by cwd.
- `agentctl doctor --repair-native-sessions` records the session for
  live lanes that lack one, from the same pinned observation, and
  refuses to guess among several held sessions.
- A restore killed midway leaves a partial report naming the lanes
  already revived; a restarted restore in the same boot finishes the
  remaining lanes without reviving the finished ones again, and the
  final report is the boot's completed report.
- A retained revive candidate whose Muse is alive in its pane with
  the recorded session is published by `agentctl revive`; a candidate
  whose live session differs, or that cannot be proved, is refused
  with the journal retained.
- No new persisted record field: old binaries keep reading records
  written by this build (reported to the coordinator before gating).
- Focused Rust and Python suites and parity tests pass; the harness
  support matrix and the team-restore skill match the code.

# Notes

[watermelon-full-preview] Lane mu-restorefix/1934 on devbig014,
coordinated by coordb. Priority high: two teams' boot restores failed
tonight. Related: #1932 (hooks trust dialog, lane mu-hookstrust),
#916 muse-native-session-id, #948, #899, #1138, #975.

Python edition: no mirror of this change. The tracked launchers run
the Rust edition, the Python edition has no restore command, and its
revive keeps the manual retire-stale-registration plus adopt
recovery; the harness support matrix records this Rust-only scope.
Session attribution was tightened after review (a held lock counts
only when it names this lane's runtime pid, with Muse's own
per-pid store report preferred and pid reuse rejected by full
process identity), so another lane's session is never recorded.

# Resolution

[watermelon-full-preview] Landed as c8591bd3 on main
(verified-overlap landing; replayed tree matched the verified tree).
Muse lanes record their native session at start, adopt and promote
from the pinned runtime (the session's own lock must name this
lane's runtime pid; Muse's per-pid store report is preferred and
pid reuse is rejected by full process identity), with
`doctor --repair-native-sessions` as the backfill for live lanes.
Restore writes a partial report as it goes and resumes from it in
the same boot epoch; the units keep TimeoutStartSec=1800 as a
per-attempt watchdog. A retained revive candidate whose harness
lives in its pane on the recorded session is pinned and published;
anything else is refused. No new record field was introduced.
Follow-ups: #1945 (stale-lock pid reuse), #1992, #2013.
