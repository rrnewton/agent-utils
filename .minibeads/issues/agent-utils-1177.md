---
title: "abort-create-clone-branches: recover --abort-create refuses submodule branches that git clone made and that match the remote"
status: closed
priority: 2
issue_type: bug
assignee: mu-createlock
created_at: 2026-11-10T01:40:00.000000000+00:00
updated_at: 2026-11-10T02:21:15.000000000+00:00
closed_at: 2026-11-10T02:21:15.000000000+00:00
---

# Description

`wrkslots recover --slot S --abort-create` refuses when an initialized submodule has a local branch whose commit
is outside the submodule's HEAD, for example:

    REFUSED: cannot abort checkout with initialized submodule third-party/rr whose local branch carries
    116719af... outside its HEAD; preserve it for inspection

Those branches are created by `git clone` itself during `git submodule update --init --recursive` (a
post-provision hook): each is the remote's default branch, equal to `origin/<default>`, with no commit outside
the remotes. So any create interrupted after the submodule hook needs a human to delete every clone-made branch
(three in one consumer checkout: `third-party/rr` master, `agent-utils/ext/minibeads/.claude_template` main,
`agent-utils/ext/minibeads/beads` main). Until someone does, the leftover CREATE journal blocks every
`wrkslots adopt` on the host.

# Design

A submodule branch counts as preserved, not unique work, when every one of its commits is reachable from that
submodule's remote-tracking refs (`git rev-list --count <branch> --not --remotes` is 0). A branch with any commit
absent from every remote still refuses, and the refusal names it. Apply the same rule at every submodule depth,
in both editions if the Rust edition has this path.

# Acceptance Criteria

- An interrupted create with a nested submodule whose clone-made default branch matches its remote: the abort
  succeeds.
- An interrupted create with a submodule branch carrying a local commit that is on no remote: the abort still
  refuses and names the branch and commit.
- Both editions agree (a differential case, if the Rust edition implements abort-create).

# Notes

Reported by a consumer harness (2026-11-09; its CREATE journal and per-branch facts were kept on that host).
Requested by the lead coordinator for mu-createlock after #1124 (same area: create journals and recovery).

## Resolution

Delivered by mu-createlock (opus 5.5), Python edition only: the Rust edition
does not implement `recover --abort-create` (its recover serves only
`--abort-import`), so there is no differential case. The initialized-submodule
check before an abort now treats a local branch outside the submodule's HEAD
as preserved when `git rev-list --count <branch> --not --remotes` is 0, at
every submodule depth. A branch with any commit absent from every
remote-tracking ref still refuses, and the refusal now names the branch as
well as the commit. Tests: an interrupted create whose nested leaf's
clone-made main sits past the gitlink on origin/main aborts (and fails
before the fix with the reported refusal); a leaf branch carrying a commit on
no remote, beside that clone-made main, still refuses naming the branch and
commit and leaves the journal and slot for inspection. The user guide's
abort paragraph states the rule.
