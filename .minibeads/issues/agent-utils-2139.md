---
title: 'python-editions-archive-and-parity-retirement: archive the Python editions with equivalence metadata and retire parity checks'
status: open
priority: 1
issue_type: task
labels:
- rust
created_at: 2026-11-22T18:20:00-05:00
updated_at: 2026-11-22T18:20:00-05:00
---

# Description

Owner direction: the Python editions are secondary and deprecated, and no cycles go to keeping them at parity with the Rust editions. Archive what exists with clear metadata on which version each was equivalent to, so the work can be picked up again if wanted.

# Design

- One archive scheme for every Python edition (tag or archive directory plus a metadata file): archived-at commit, the Rust commit and version it was last equivalent to, and its issue.
- Retire each Python-vs-Rust differential and parity test from the gates, one tool at a time.
- Coverage rule: before retiring a differential, name the Rust-side tests that cover the same behaviour; where the differential is the only coverage, convert its goldens into Rust-only regression tests.
- The gate and validate scripts stay until local-ci-hub and the gate move to Rust.
- Consumers that pin older readers keep working; no persisted-format change without notice.

# Acceptance Criteria

- Every Python edition archived with its metadata; no gate step compares Python and Rust editions; no Rust behaviour lost its only test.

# Notes

Owner request; filed by the coordinator.
