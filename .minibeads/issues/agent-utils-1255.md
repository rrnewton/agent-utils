---
title: 'wrkslots-private-history-refusal: removal names unpushed private submodule history instead of calling it uncommitted changes'
status: closed
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T00:50:00+00:00
updated_at: 2026-11-11T00:58:47+00:00
closed_at: 2026-11-11T00:58:47+00:00
---

# Description

[opus 5.5] A consumer's slot retirement is blocked by a misleading refusal. Its checkout holds a self-sourced submodule that is clean: no status, diff or untracked lines. But the submodule has commits that aren't proven durable (in the reported case, 483 refs and 52 commits off HEAD that no remote is known to hold). `wrkslots remove` treats the unproven private history as dirty and refuses with "submodule NAME has uncommitted changes and its source no longer declares it; preserve the slot and commit or publish that work by hand".

The refusal is correct, since the history may be the only copy. The message is wrong, though. The operator looks for uncommitted changes, finds none, and has no documented way forward.

# Design

- Keep the two facts apart in the salvage facts: uncommitted work (status, diff, untracked) versus unproven private history (commits or refs not shown to exist on any remote and not bundled).
- When removal refuses on private history, say what it found: "unpushed private history in submodule NAME: N commits on M refs not on any remote", list a few of them, and name the command that preserves them.
- Document a salvage path: a way to preserve that history before removal, for example pushing it to a backup ref under `refs/rescue/` on a configured remote, or bundling it into the slot's salvage archive as clean submodules already are (#871). After that, removal proceeds without hand edits. The operator runs it explicitly; removal never pushes on its own.
- Rust parity: the Rust edition's refusal text and facts match.

# Acceptance Criteria

- A clean self-sourced submodule with unproven private history refuses with a message naming private history and its counts, never "uncommitted changes".
- A submodule with real uncommitted work still refuses with the uncommitted-changes message.
- The documented salvage step preserves the history, and a following removal succeeds. Tests cover both, in both editions.

# Notes

Reported by a consumer team on 2026-11-10 (redacted). The refusal must stay; only the diagnosis and the way forward change.

Raised to P0 by the lead on 2026-11-10 after the data-loss finding below.

## Resolution

[opus 5.5] Fixed in Python; the Rust edition never reaches this case (see the spec below).

This is a CORRECTNESS fix, not only a message fix. Before it, a submodule the primary no longer declares (self-sourced, #910) counted its own module repository as durable. Removal deletes that repository, so every own ref, tags and local branches included, "proved" its commits kept. The result: a commit on a local-only branch or tag of such a submodule was DELETED SILENTLY, with an `already-published` / `remote-verified` receipt and no bundle (reproduced with both a side branch and a local annotated tag). Only commits that a reflog alone, or nothing, held were caught, and they refused with the wrong text, "has uncommitted changes". That text is what the consumer saw. Their audit found 52 tag-held commits; under the old code those already counted as kept, so their refusal came from reflog-only or unreferenced commits, which the audit did not inspect.

Now:
- The self-sourced module store is no longer durable. A commit counts as kept only when the recorded remote's advertised branches or tags reach it (`ls-remote` heads and tags, plus a commits-only fetch of advertised tips missing locally), or a local mirror outside the slot holds it.
- The facts are split into uncommitted work and unpushed private history. Uncommitted work keeps the old refusal byte for byte; private history gets a counted refusal naming refs and the salvage step.
- `remove --archive-private-history` (also on `remove-agent-batch` and `retire-pending`) keeps that history in a verified local bundle, never pushed.
- The recheck before deletion derives a self-sourced submodule's facts with its private state, as salvage did.

Removal is therefore STRICTER for truly local commits: a slot that used to be removed while losing them now refuses until the operator adds the flag or publishes them. Validated by 8 new lifecycle cases (7 fixtures, one parametrized) plus the existing #871/#910/#859 and drift-submodule cases. Unverified: the consumer's own tree; nothing here touched it.

## Rust parity spec

For the Rust remove port (wrkslotsd remove_write.rs refuses any slot with initialized submodules today: "submodule salvage stays with the Python edition for now"). Python reference: py/wrkslots/cli.py, functions named below.

**Refusal strings** (exit 3, "REFUSED: " prefix as for every refusal):
- Uncommitted work (unchanged): `submodule NAME has uncommitted changes and its source no longer declares it; preserve the slot and commit or publish that work by hand`
- Private history only (`_private_history_refusal`): `unpushed private history in submodule NAME: FOUND[UNCHECKED]. Its source no longer declares it, so removal would delete the only copy. remedy: keep that history in a verified local bundle in the salvage archive, never pushed, by rerunning 'wrkslots remove SLOT' with the same options plus --archive-private-history; or publish it by hand and rerun remove`
  - FOUND with refs: `N commit(s) on M ref(s) not on any remote (R1, R2, ... up to 5[ and K more])`, then `; J of those commits only a reflog entry or no name at all holds` when J > 0. Singular `1 commit`, `1 ref`. Refs are sorted by full name.
  - FOUND with no refs: `N commit(s) not on any remote, which only reflog entries or no name at all hold`
  - UNCHECKED, when the remote could not be asked: ` (remote ORIGIN could not be asked, so none of its branches or tags counted: REASON)`

**Facts** (`_SalvageCandidate`, private-state salvage only, `_salvage_candidate_uncached` with keep_private_state):
- `uncommitted_work` is true when the status is non-empty OR the index tree or the worktree-through-index tree differs from HEAD and from the salvage tree (`_index_state_trees`).
- `private_history` (`_PrivateHistory`): `tips` (extra salvage parents), `commits` (count), `refs` (local ref -> commit, for refs whose peeled tip is a private commit), `unreferenced` (private commits no such ref reaches), `remote_unchecked`.
- `dirty` = either. Decision in `_salvage_agent_slot`: self-sourced and dirty -> uncommitted (or no private history) refuses with the old text; private history only refuses with the new text unless --archive-private-history.

**Coverage** (`_submodule_private_history`, `_durable_submodule_repositories`, `_GitVcs.remote_advertised_covering`):
1. Positives: both object names of every reflog entry (common and per-worktree logs), every pseudoref, every ref, and every commit object (`cat-file --batch-all-objects`) except commits whose committer email is `wrkslots@localhost`.
2. Negatives: the salvage commit (HEAD when clean), plus every ref of each durable repository. Durable = the recorded source repository UNLESS it lies under what removal deletes (the slot dir, canonical or fenced, or any checkout's absolute git dir; self-sourced always does), plus a local-path remote (configured or insteadOf-rewritten) outside those. Durable object dirs serve as alternates.
3. tips = commits positives reach and negatives don't, minus parents among them.
4. Self-sourced only, when tips remain: `ls-remote --refs URL 'refs/heads/*' 'refs/tags/*'` at the recorded URL (network_authority when aliased), in an isolated repository over the module's objects with the user's config ignored. Each advertised oid present locally is peeled with `cat-file --batch-check` of `OID^{commit}` (commit -> itself, annotated tag -> its commit; absent or non-commit skipped), and those commits are negatives. Tips still unreached by them, if any advertised oid is missing locally: fetch the missing refs, `--filter=tree:0 --no-tags`, into an EMPTY store made inside the module's own git dir (`_fetched_proof_store`; inventory must equal the advertisement; no lazy fetch). A remaining tip present there is covered. Any refusal -> nothing covered, reason kept in remote_unchecked.
5. Memo: `_REMOTE_CONTAINMENT_MEMO` key (objects dir, "covering\0"+URL, sha256 of sorted tips, False), 300 s, refusals memoized too; prefetched before the registry lock by `_prefetch_remote_containment`.
6. Recount with covered commits as extra negatives; `commits` = rev-list tips ^negatives.

**Bundle contract** (`--archive-private-history`): the salvage commit is HEAD's salvage commit with every private tip as an extra parent (fixed identity and date = record heartbeat, message lists `kept: SHA`). It is kept through the never-push local archive (`_archive_salvage_locally`, shared bundle, complete history, fresh bare clone + exact ref + full fsck). The receipt says `disposition: archived-local` and `archive_reason: private-history-kept`. The stderr warning names `N commits: REF SHA12, ...`. Nothing is pushed; uncommitted work still refuses. The prefetch makes no bundle for a candidate that will refuse. The recheck (`_assert_salvage_still_matches`) derives self-sourced submodules with keep_private_state, so it gets the same commit. Restore: `git fetch BUNDLE ARCHIVE_REF:refs/heads/wrkslots-restored`; the parents after the first are the kept tips. An annotated tag is kept as its commit only.

**Fixtures** (py/wrkslots/tests/test_lifecycle.py, built on `_slot_with_primary_dropped_leaf`):
1. `test_remove_keeps_nothing_extra_for_history_under_remote_advertised_tags`: 2 commits held only by a lightweight and an annotated tag the remote advertises -> removed, already-published.
2. `test_remove_refuses_unpushed_private_history_with_counts`: a local branch and a local annotated tag, 2 commits on 2 refs -> exact refusal, no "uncommitted".
3. `test_remove_refuses_private_history_no_ref_names[reflog|nothing]`: a deleted branch, its commit held by the reflog or (after expiry) by nothing -> "1 commit not on any remote, which only reflog entries or no name at all hold".
4. `test_archive_private_history_keeps_it_in_a_verified_bundle`: fixture 2 + the flag -> archived-local, private-history-kept, remote refs unchanged, bundle reaches both, salvage parents after the first == the 2 tips.
5. `test_archive_private_history_still_refuses_uncommitted_submodule_work`: fixture 2 + an uncommitted edit + the flag -> the old uncommitted text.
6. `test_remove_counts_history_a_remote_tip_missing_locally_reaches`: a local branch at a commit that only the remote's newer, locally absent, branch tip reaches -> removed via the commits-only fetch.
7. `test_remove_no_longer_drops_a_local_side_branch`: the data-loss case -> "1 commit on 1 ref not on any remote (refs/heads/side)", slot intact.
