---
title: "mq-close-comment: map every landed commit in the merge-queue closing comment"
status: open
priority: 2
issue_type: feature
assignee: ac-mqclose
created_at: 2026-11-09T23:11:00.000000000+00:00
updated_at: 2026-11-09T23:11:00.000000000+00:00
---

# Description

When the merge queue lands a pull request by rebase, the hosting service shows the pull
request CLOSED with no merge time, and the queue's closing comment names only the final
landed SHA. A coordinator reading a landed multi-commit pull request saw only the first
commit's subject next to that one SHA, read the landing as partial, and nearly re-applied
a commit that was already on the base branch.

Make the closing comment prove what landed:

- map each original pull-request commit to its landed SHA (for example
  `7e3cbe79 -> 718d6431, f9284ed9 -> 9d30d450`), matching by patch-id when the rebase
  rewrites the SHAs;
- say "all N commits landed", or list which commits did not land and why (squashed into
  another commit, dropped because the base already carries the patch, or not present in
  the landed range);
- name the landing batch (the changes landed together and the resulting tip).

# Design

The closing comment is written by `close_comment` in `rs/merge-queue/src/sync.rs` and
posted by `close_postponed`. The commit mapping is computed where the landed commits are
known (at landing, from the candidate's per-change prefixes) and stored on the queue
item, so the closer only formats it. Patch identity comes from `git patch-id --stable`.
Items landed before this change carry no mapping and keep the previous comment.

# Acceptance Criteria

- A 1-commit pull request's closing comment maps its one commit and says all 1 commits
  landed, naming the batch.
- A multi-commit pull request's closing comment maps every original commit to its landed
  SHA by patch-id and says all N commits landed.
- A squashed or partially dropped pull request's comment lists the commits that did not
  land as their own commits, with the reason for each.
- Tests cover the comment text for all three cases.
