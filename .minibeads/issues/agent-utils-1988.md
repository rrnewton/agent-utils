---
title: 'wrkslots-stale-module-repo-pin: slot creation fails when the shared module repo lacks the pinned submodule commit'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-23T12:00:00+00:00
updated_at: 2026-11-23T12:00:00+00:00
---

# Description

[watermelon-full-preview] Relayed 2026-11-22: after a submodule repin, creating a new slot fails in post-provision hook 1 with "upload-pack: not our ref <sha>". The shared repository's submodule URL points at the local module repository (`.git/modules/<name>`), whose refs are stale and lack the newly pinned commit; nothing refreshes that module repository, so every future repin breaks slot creation again. The workaround was to fetch the host mirror's main into the module repository by hand.

# Design

In the shared-submodule path (`_share_checkout_submodules`), before adding the linked worktree, ensure the pinned gitlink commit is present in the module repository: check with `cat-file -e`, and if missing fetch that exact commit from the configured submodule URL resolved against the source checkout under wrkslots' clean git environment, falling back to the URL's default refs, and refuse with a named remedy before hooks run if it is still missing. Refreshing the mirror itself (option b) is the complementary layer outside wrkslots and is noted here, not implemented. No new persisted field is introduced; any new persisted field would be reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism) or not persisted.

# Acceptance Criteria

- A slot whose superproject pin is newer than the module repository's refs provisions successfully, fetching only the pinned commit.
- A pin unreachable from the configured URL refuses before any post-provision hook runs, with a named remedy.
- Existing shared-submodule suites pass, in both editions where the surface exists.

# Notes

Approved plan direction recorded from the coordinator before coding. Whether current main already changes this behaviour is checked before the fix and recorded in the resolution.
