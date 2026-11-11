---
title: 'wrkslots-declared-submodule-local-refs: removal keeps or refuses local branches and tags made inside a declared submodule'
status: closed
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T01:20:00+00:00
updated_at: 2026-11-11T01:31:31+00:00
closed_at: 2026-11-11T01:31:31+00:00
---

# Description

[opus 5.5] Found while fixing #1255 wrkslots-private-history-refusal. For a submodule its source still declares, a normal (non-drift) removal never computes private history (keep_private_state is false there). Its HEAD and worktree are salvaged, but local branches and tags created inside the slot's submodule store, and not on any remote, are deleted without a refusal or a bundle.

# Design

- Apply the #1255 rule to declared submodules: commits on local refs not covered by the recorded remote's advertised heads or tags are private history.
- The same precise refusal, and the same `--archive-private-history` bundle salvage.
- Keep the declared-submodule fast path when there are no local-only refs (no network call when the refs are all covered locally).
- Rust parity: hand the spec to the Rust remove-parity work, as for #1255.

# Acceptance Criteria

- A declared submodule with a local-only branch or tag refuses with the private-history message, and removes with `--archive-private-history`, keeping a verified bundle.
- A declared submodule whose refs are all covered removes as before.
- Tests count refs and commits; none use time.

# Notes

Follow-up to #1255. Related: #1256 wrkslots-removal-ref-inventory.

Raised to P0 by the lead on 2026-11-10 (data loss, same treatment as #1255). Lead decisions the same day: refuse (not auto-bundle), consistent with #1255; stale remote-tracking refs count as private, and the refusal names them; batch removals report every refusing slot.

## Resolution

[opus 5.5] Fixed in Python. This is a CORRECTNESS fix. Before it, a normal (non-drift) removal salvaged only a declared submodule's HEAD and worktree. Commits on local branches or tags, in the stash, or on stale remote-tracking refs of the slot's own copy of the submodule repository were deleted with the slot, with no refusal and no bundle, whenever neither the source repository nor the remote held them.

Now:
- Every declared, undrifted submodule gets a private-history check before anything is salvaged (_declared_submodule_private_history). It uses the #1255 rule: durable = the source submodule repository plus a local mirror outside the slot; then the recorded remote's advertised branches and tags. The remote is asked only about commits the local repositories leave, so the common case makes no network call beyond today's HEAD check.
- Private history refuses with the #1255 counted message ("Removal deletes the slot's own copy of its repository, so it would delete the only copy."). Commits that only stale remote-tracking refs reach are named: "N of those commits only on stale refs/remotes/origin/X, deleted upstream" (or "moved upstream").
- `--archive-private-history` re-derives that submodule with its private state and keeps it in a verified local bundle, never pushed (archive_reason private-history-kept). The recheck before deletion derives it again from that receipt reason.
- Remote coverage now applies to every submodule, so a drifted checkout's submodules no longer bundle commits the remote already advertises.
- Batch semantics checked, unchanged: remove-agent-batch and retire-pending report the refusal per slot and go on with the others. A new test pins it (two refusing slots + one clean slot).

Removal is STRICTER: a slot whose declared submodule holds such commits refuses until the operator adds the flag or publishes them.

## Rust parity spec (addendum to #1255's)

- Declared check: in _salvage_agent_slot, for each submodule candidate that is not never-push (not drifted, not self-sourced, not unlisted), run _submodule_private_history with kept = the candidate's salvage commit and durable = _durable_submodule_repositories (source + mirror), plus remote cover. A result refuses at once, before any salvage, with `_private_history_refusal(..., declared=True)`. Its second sentence is `Removal deletes the slot's own copy of its repository, so it would delete the only copy.`; the rest is as in #1255.
- Stale remote-tracking refs (_private_history_summary): when the remote answered, each private ref `refs/remotes/<recorded remote>/<name>` (except HEAD) is stale. It is `moved upstream` if the remote advertises refs/heads/<name>, else `deleted upstream`. stale_only = commits reachable from the stale refs' tips minus commits reachable from the other private refs (both walks exclude the negatives). The refusal appends `; K of those commits only on stale REF, REASON` for one ref, or `; K of those commits only on stale remote-tracking refs (REF, REASON; REF, REASON[; and N more])` for several (up to 5 shown), and only when K > 0.
- Coverage memo: the _RemoteContainmentEvidence entry also stores the advertised branch names (`extra`).
- With the flag: the candidate becomes never-push, it is re-derived with keep_private_state (index trees + tips as extra parents), and it is archived locally with `archive_reason: private-history-kept`. _assert_salvage_still_matches uses keep_private_state when the receipt's archive_reason is private-history-kept.
- Prefetch (_prefetch_remote_containment) runs the same check before the lock; it skips the bundle for a refusing candidate and pre-makes it with the flag.
- Fixtures (test_lifecycle.py, `_dead_owner_slot_with_declared_component`):
  1. `test_remove_refuses_private_history_in_a_declared_submodule`: a local branch + a local annotated tag -> exact refusal; component remote untouched.
  2. `test_archive_private_history_bundles_a_declared_submodule`: fixture 1 + flag -> archived-local, private-history-kept, salvage parents after the first == the 2 tips.
  3. `test_remove_counts_declared_submodule_history_the_source_holds`: the commit is on a source-repository branch only -> removed.
  4. `test_remove_counts_a_declared_submodule_tag_the_remote_advertises`: -> removed.
  5. `test_remove_counts_a_declared_submodule_stash_as_private_history`: -> "2 commits on 1 ref not on any remote (refs/stash)" (the stash worktree commit and its index-commit parent).
  6. `test_remove_names_commits_only_a_stale_remote_tracking_ref_holds`: a 2-commit branch fetched and then deleted upstream -> "...(refs/remotes/origin/feature-x); 2 of those commits only on stale refs/remotes/origin/feature-x, deleted upstream."
  7. `test_remove_agent_batch_reports_every_private_history_refusal`: 3 slots, 2 refusing -> rc 1, requested=3 removed=1 refused=2.
