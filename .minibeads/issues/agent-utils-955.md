---
title: 'wrkslots-remove-nested-fingerprint-drift: remove says a nested submodule changed after salvage when nothing touched the dead slot'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-06T19:14:11.000000000+00:00
updated_at: 2026-11-06T19:14:11.000000000+00:00
---

# Description

On two dead consumer slots, remove refused with "checkout <superproject>/<sub>/third-party/<subsub> changed after salvage; preserve it and rerun remove". Nothing touches those slots. In both, the nested submodule's superproject checkout differs from its recorded branch, so the fingerprint taken before salvage and the one at recheck seem to differ for a reason other than real change.

# Design

- Reproduce with a nested submodule whose checked-out commit differs from its recorded branch. Diff the two fingerprints field by field to find what changes. Likely suspects: a ref resolved through the superproject, index stat data refreshed by a git command during salvage, or untracked-cache or fsmonitor state.
- Make the fingerprint depend only on content that salvage preserves. Remove must still refuse when the checkout really changed.

# Acceptance Criteria

- A test: a dead slot with a nested submodule off its recorded branch salvages and removes without the "changed after salvage" refusal.
- A test: a real edit between salvage and recheck is still refused.

# Notes

Reported 2026-11-06 by the lead coordinator from a consumer registry. Part 2 of 3 with #954 and #956.
