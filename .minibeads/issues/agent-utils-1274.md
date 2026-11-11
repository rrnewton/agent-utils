---
title: 'empty-commit-trailers: replay drops an empty commit that only carries trailers'
status: closed
priority: 0
issue_type: bug
created_at: 2026-11-11T07:00:00.000000000+00:00
updated_at: 2026-11-11T07:00:00.000000000+00:00
---

# Description

The consumer's census reads Test-Dropped trailers from `git log base..head` of the queue's CANDIDATE.

The queue's replay (combine = rebase; rs/merge-queue/src/git.rs cherry_pick_with) drops any commit whose result equals the target's tree, just as git rebase drops an empty commit.

So an EMPTY commit whose only job is to carry Test-Dropped trailers (a documented consumer pattern, with precedent on their main) is silently dropped when the PR's base isn't main. The census then refuses the "undeclared" drops; one consumer PR hit exactly this. Their workaround tonight: put the trailers on a non-empty commit.

# Design

In `rs/merge-queue` (replay in `git.rs`):

- Either keep empty commits that carry trailers in the replay (or carry their trailers onto the next commit), or refuse at intake with a named message ("empty commit X carries trailers that a replay would drop"), so authors learn at submit time rather than after a 15-minute red. Pick the one that is simplest and safe; keeping trailer-carrying empty commits (cherry-pick --allow-empty / --keep-redundant-commits when the message has trailers) seems most faithful.

# Acceptance Criteria

- A trailer-carrying empty commit survives a replay (its trailers still read from the candidate's log), or intake refuses it with the named message.
- `cargo test -p merge-queue` (all targets) passes, including the new case.

# Notes

Reported 2026-11-11. Fix at owner tier after #1273 probe-no-counted-tests.
