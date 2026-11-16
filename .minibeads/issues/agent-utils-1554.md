---
title: 'eden-remove-and-absent-rows: Eden validate rows must not block git creates or absent-row recovery'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-15T11:35:00.000000000+00:00
updated_at: 2026-11-16T00:52:59.000000000+00:00
closed_at: 2026-11-16T00:52:59.000000000+00:00
---

# Description

Two defects from an Eden isolation test on a shared host, where two
Eden validate rows (storage already absent, `vcs=eden`, stored
repository = the absolute Eden backing repository outside the
project) were left in a registry:

1. Every `wrkslots create` on that registry refused, including plain
   git agent slots, with "stored absolute repository path must name
   one direct sibling of the project root, not the project root or
   another external path: <backing repo>". Some create-time scan over
   existing rows resolved an Eden checkout's repository through the
   Git sibling rule, without the `checkout.vcs == "eden"` skip that
   `_registry_storage_inconsistencies` and the cross-repository checks
   already have.
2. `recover-absent-validate-rows` (plan mode) refused the same rows
   with the same sibling message, so the stuck rows could not be
   disposed of even though their storage was already gone.

A third defect from the same test is split out as a later commit:
`wrkslots remove` of an Eden validate slot runs `git ls-files` on the
non-git Eden checkout. It is not part of this change.

Related: #1204 (Eden checkouts; backing repos live outside the
project by design), #1558 (Eden create failure cleanup).

# Design

Part A (this change):

- Every scan over existing rows that resolves a checkout's repository
  for Git purposes skips `vcs=eden` checkouts; only the Git repository
  resolution is skipped. Checkout-path and slot-path overlap checks
  still cover Eden rows, so a new slot can never land on an Eden mount
  or slot directory.
- `recover-absent-validate-rows` accepts a `vcs=eden` checkout's
  backing repository: it is validated as an Eden backing (a canonical
  absolute path) by `_eden_backing_repository`, never by the Git
  sibling rule, and no Git registration check is run for it. The
  absent-storage proof is unchanged, so an Eden row whose storage is
  still present is not treated as absent.
- When absent rows are disposed of, their stale bind records in
  `.worktrees-image.json` are dropped through the existing
  worktrees-image code (`load_bind_records` / `forget_bind`), named
  slots only.
- A git row with a non-sibling repository path still refuses, exactly
  as before.

# Acceptance Criteria

- Fixture: an ACTIVE validate row with one `vcs=eden` checkout, an
  absolute non-sibling backing repository, and absent storage; a plain
  git `create` then succeeds. Before the fix it refuses with the
  sibling message above.
- A create whose target overlaps an Eden row's slot path still
  refuses.
- `recover-absent-validate-rows` storage validation accepts the Eden
  row; a git row with a non-sibling repository still refuses; an Eden
  row with storage present is not treated as absent.
- Disposing of absent rows drops only their own stale bind records.
- Stub fixtures only; no real Eden is touched, and no other team's
  registry is ever pointed at.

# Notes

## Resolution

Landed in four parts (mu-edenfail, each gated before land-main). 528631d2 part A: existing-row scans skip only Git repository resolution for vcs=eden checkouts, while path and slot overlap checks still cover Eden rows; absent-row recovery accepts Eden backings as canonical absolute paths; stale bind records are dropped on disposal. 0a574796 part A2: archived Eden checkouts validate at their Eden layout path in both the Python and Rust archive validators. b12aeb5c part A3: a refused recover-absent --apply that wrote nothing releases its own journal; refused replays keep theirs. 4b1d54f8 part B: plain remove of an Eden validate slot goes through the Eden remove sequence with its safety proof and never takes the Git path fence, handoff scan or registration checks. Each fix has a test that fails on the parent code.
