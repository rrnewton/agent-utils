---
title: 'wrkslots-soak-image-remove-fallback: the soak wrapper refuses remove for image-backed slots instead of using the Python edition'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-07T22:50:00.000000000+00:00
updated_at: 2026-11-07T22:50:00.000000000+00:00
---

# Description

Under `WRKSLOTS_ENGINE=rust-soak` (the soak wrapper that runs Rust-covered commands in Rust and the rest in Python), `wrkslots remove SLOT` on an image-backed slot exits with "slot SLOT is image-backed; finish stays with the Python edition for image-backed slots". It does not hand the call to the Python edition, so operators must run py/wrkslots/__main__.py directly. `unhold` is similar: its help says "fixture registries only until cutover".

# Design

In soak mode, treat "stays with the Python edition" as not covered and dispatch that invocation to the Python CLI with the usual stderr note, as for other uncovered commands. Log it in the soak log so coverage gaps stay visible.

# Acceptance Criteria

- `remove` of an image-backed slot through the soak wrapper succeeds via Python and logs the fallback.
- A test covers the dispatch decision.

# Notes

Found in the 2026-11-07 stale-slot cleanup, where four removals needed the Python CLI by hand.
