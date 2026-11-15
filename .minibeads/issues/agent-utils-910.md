---
title: 'wrkslots-undeclared-nested-submodule: removal accepts or bundles a nested submodule the primary no longer declares'
status: closed
priority: 1
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-05T22:24:35.078892251+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
closed_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

Removing a dead slot refuses outright when the slot's checkout declares a nested submodule that the primary repository no longer has. A consumer team hit this on 2026-11-05 after a repository split: the slot's older superproject still declared third-party submodules that its main had dropped. Removal refused with:

    REFUSED: source submodule is unavailable for <repo>/third-party/<lib>: .../third-party/<lib>. Preserve the slot until its remote authority can be established

A precursor refusal, "source submodule is not a Git root", came for nested submodules the slot never initialised. Every nested HEAD was verifiably on the remote. The team overrode by hand.

# Design

- A nested submodule the primary no longer declares is handled by evidence, not refused:
  - If its local HEAD and refs are on a remote (checked by `git ls-remote` against its recorded URL), accept it as salvaged.
  - Otherwise bundle its local refs into the salvage directory.
- An uninitialised submodule (no .git, nothing checked out) has nothing to salvage. Skip it with a note; never require it to be a Git root.
- Refuse only when there are commits that are neither on a remote nor bundleable. The refusal names them.

# Acceptance Criteria

- Test: a slot whose superproject declares a submodule the primary dropped, with the HEAD on a fake remote. Removal proceeds and records "remote-verified".
- Test: the same, but with local-only commits. They are bundled and removal proceeds.
- Test: an uninitialised nested submodule is skipped, not refused.
- Test: unreachable commits that cannot be bundled still refuse, naming them.

# Notes

Reported by the lead on 2026-11-05 at 5:21 PM ET (P1, wrkslots reclaim on the lead host; about 48 GB was blocked across two dead slots). Related: #864 (nested unlisted module salvage). Assigned to the dedicated wrkslots lane.

## Resolution

[watermelon-full-preview] Delivered on main by fae1ebce0ccab8749e12fd0ecec2fc42741ada50 (with #871 wrkslots-unlisted-module-salvage): a nested submodule the primary no longer declares, or never initialised, is judged by the slot copy's own recorded remote instead of refusing. Acceptance tests in py/wrkslots/tests/test_lifecycle.py: test_remove_accepts_nested_submodule_the_primary_dropped_when_remote_verified (remote-verified receipt), test_remove_bundles_nested_submodule_the_primary_dropped_when_unpublished, and test_remove_skips_an_uninitialised_nested_submodule. The refuse-naming criterion is covered strictly by #1255 wrkslots-private-history-refusal (67d1b0d43c48d0d28c934008f38b2f963d72b1c0): test_remove_refuses_unpushed_private_history_with_counts and test_remove_refuses_private_history_no_ref_names refuse on this fixture naming counts and refs, including commits only a reflog or no name holds. Note the deliberate stricter semantics from #1255: private history on side refs refuses by default and bundles only under --archive-private-history, rather than bundling automatically. Closed during GitHub triage (Request 098) alongside GitHub #300 and #301.
