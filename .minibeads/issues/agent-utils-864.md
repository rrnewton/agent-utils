---
title: 'wrkslots-unlisted-module-salvage: recurse into nested module repos and verify big bundles outside the lock'
status: open
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T04:10:15.603371028+00:00
updated_at: 2026-11-05T04:10:15.603371028+00:00
---

# Description

Follow-up to #859 wrkslots-unlisted-module-salvage (closed). A retry on the lead host on 2026-11-04, using a source export that contains #859, freed 0 bytes. All four dead-owner slots were still refused. About 4 slots of reclaim (tens of GB) stay blocked. Not urgent.

There are three gaps:

1. **Nested unlisted modules are not reached.**
   - The refused repository was a submodule of a submodule of a submodule: `<superproject>/.git/worktrees/<slot>/modules/<sub>/modules/third-party/<lib>`, in an 8.3 GB slot.
   - `_unlisted_submodule_checkouts` / `_assert_listed_module_repositories` look only one module level deep, so the salvage never bundles it.
   - The salvage directory had 5 bundles, and none of them was the nested module.
2. **fsck of a large salvage bundle trips the lock budget.**
   - After drift archiving passed the unlisted stage, `git fsck --full --strict` of a salvage bundle used the whole 60-second remove-agent-batch budget (exit 3).
   - Big bundles cannot be verified inside the lock as it stands.
3. **Detached HEAD disables salvage.**
   - `salvage_unlisted_modules=owner_proven_dead and not detached`.
   - That slot's superproject HEAD was detached from its recorded branch, so salvage was off even though the owner was proven dead.
   - Check whether `not detached` is still required once the drifted checkout has been archived (`--archive-drifted-checkouts`). If it is, say why in the refusal text.

4. **A nested checkout that tracks zero files breaks drift archiving.**
   - Seen again on three later removals.
   - `wrkslots remove --archive-drifted-checkouts` runs `git add -u -- . ':(exclude…)'` in every checkout.
   - In a checkout whose commit has an empty tree (an "initialize review base" commit), Git dies with `pathspec '.' did not match any file(s) known to git`, and the removal is refused.

These are out of scope here; they were seen on the same retry:
- Two slots were refused because the SOURCE submodule directory is empty (`source submodule is unavailable`). That needs the shared source checkout to be populated; it is not a wrkslots defect.
- That slot's remote-URL refusal (an ssh origin that was switched to https) is #868 wrkslots-remote-url-equivalence. Its `pathspec '.'` failure is gap 4 above.

The lead's full retry note, with line references against `py/wrkslots/cli.py` at ec150b5e, is in the coordinator handoff directory as `859-retry-refusals.md`.

# Design

- Make unlisted-module discovery recursive. Walk `modules/` inside every module repository, at any depth, and give each nested repository the same listed/unlisted test and salvage bundle as a top-level one. Record the nesting path in the salvage manifest.
- Move bundle verification out of the lock. Write and fsck the bundle before taking the batch lock (or in a phase that holds no lock); take the lock only to re-check that nothing changed and to delete. If verification has to stay inside the lock, scale the budget with bundle size, and print the budget used, so a big bundle never meets a fixed 60 s.
- In drift archiving, skip `git add -u` when the checkout tracks no files (`git ls-files` is empty). Otherwise use a form that cannot fail on an empty index, for example `git add -u :/` with the excludes, and treat Git's "did not match" as nothing to stage only when the index is empty.
- Keep every existing refusal for live owners and for unpublished work.

# Acceptance Criteria

- A test builds a superproject → submodule → nested submodule fixture, makes the nested one unlisted in a dead-owner slot, and proves it is bundled, verified, and then removed (and is still refused for a live owner).
- A test (or a measured run) shows that fsck of a bundle larger than the budget's worth of work no longer exits 3 under the lock.
- A test archives a drifted dead-owner slot whose nested checkout sits at a commit with an empty tree, and the removal completes.
- The detached-HEAD case is either salvaged after drift archiving or refused with text that names the reason.
- A retry on the four slots is reported back to the coordinator, with bytes freed, or the remaining refusals quoted verbatim.

# Notes

Requested by the lead on 2026-11-04 at 11:04 PM ET. Assigned to mu-validate, queued after its current latency-test rework.
Gap 4 (a nested checkout with zero tracked files) was added on 2026-11-04 at the lead's request; it is the same salvage path.
