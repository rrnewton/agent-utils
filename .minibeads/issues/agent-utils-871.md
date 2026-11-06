---
title: 'wrkslots-unlisted-module-salvage: remove or bundle a clean nested submodule the current source no longer declares'
status: closed
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-05T06:48:32.900837659+00:00
updated_at: 2026-11-05T06:48:32.900837659+00:00
closed_at: 2026-11-06T19:18:43.000000000+00:00
---

# Description

This is a sibling of #864 for the "source submodule is unavailable" refusal that #864 scoped out. It is now in scope at the lead's request.

**The case**
- A slot was created before a consumer superproject split one of its submodules. Its nested submodule still has three third-party submodules initialized and CLEAN.
- The primary checkout's current version of that submodule no longer declares those paths. Rolling the primary back is forbidden.
- `wrkslots remove` therefore refuses: "source submodule is unavailable for `<path>` ... Preserve the slot until its remote authority can be established" (`_submodule_salvage_checkouts_uncached`, source taken from `_source_submodule_directory` in the SOURCE repository).
- Two such slots on the lead host are blocked this way, plus the two "source submodule unavailable" slots already described in #864.
- All three nested HEADs are verifiably published:
  - one on a fork branch;
  - one upstream;
  - one under a `refs/rescue/` ref.

**Pitfall**
A global `url.<base>.insteadOf` rewrite on the host sends one of those remote URLs (the `.git` spelling) to a stale local mirror that lacks the published commit. A remote-authority check that inherits the user's global Git config wrongly concludes the commit is unpublished.

# Design

When the source repository no longer declares a nested submodule that the slot still has, decide from the slot's own repository instead of refusing:

- **Accept and remove** when its worktree is clean AND its HEAD is verifiably on a remote. Check this against the URL recorded in the slot's own submodule config: `git ls-remote` / a fetch-check of HEAD, or the commit reachable from an advertised ref (including `refs/rescue/*`).
  - Run every such network check with `GIT_CONFIG_GLOBAL=/dev/null` (and no system config), so that user insteadOf rewrites cannot redirect it.
  - Record which ref proved publication in the removal receipt.
- **Otherwise salvage:** when it is clean but publication cannot be proven, write it into the salvage bundle exactly as #864 does for unlisted modules (fsck outside the lock), then remove.
- **Refuse, as today,** when the submodule's worktree is dirty, or the owner is not proven dead.

Reuse #864's recursive nested-module walk; do not add a second traversal.

# Acceptance Criteria

- A test builds a superproject whose current submodule commit drops a nested submodule that an older slot still has initialized and clean:
  - with the nested HEAD published on its remote, removal succeeds and the receipt names the proving ref;
  - with it unpublished, it is bundled, then removed;
  - with it dirty, it is refused.
- A test sets a global `insteadOf` rewrite that points the remote at a mirror lacking the commit, and shows that the check still proves publication (the global config is ignored).
- A retry on the blocked slots is reported back to the coordinator, with bytes freed, or the refusals quoted verbatim.

# Notes

Requested by the lead on 2026-11-05 at 01:48 AM ET. P2; it does not block landings. Assigned to mu-validate after #864 (it shares #864's nested walk and bundling), before #868.

Moved from mu-validate to the new Muse ultra wrkslots lane mu-wrkslots on 2026-11-05 at 5:50 PM ET (the owner lifted the Muse lane cap; the lead asked for a dedicated wrkslots lane).

## Resolution

Landed in the mu-wrkslots stack at 84585684 (clean cases with #910). A self-sourced listed submodule with uncommitted changes is refused rather than bundled, and publication is still proven at the recorded URL with a global insteadOf rewrite pointing at a stale mirror, the receipt naming the proving refs. The blocked-slot retry remains a coordinator follow-up.
