---
title: 'wrkslots-rust-census-handle-matching: host-wide census and Python-exact handle matching for the Rust writer'
status: open
priority: 2
issue_type: task
created_at: 2026-11-24T00:00:00-05:00
updated_at: 2026-11-24T00:00:00-05:00
---

# Description

Until this lands, the proven absent-validate `--apply` path in Rust
refuses every row, because a Rust writer must never write with narrower
safety than Python. This is the sequel that lets `6b81a23c`'s refusal be
removed: port Python's host-wide process census
(`_assert_absent_validate_processes_unrelated`, mount/cwd/open-file/maps/
privileged-path use judged as a retained handle where the identical
evidence is inconclusive) and `_retained_handles_for_absent_rows`'
file-identity handle matching (normalized symlink-resolved checkout
paths; unsafe or unreadable handle directories/files fail closed) to
Rust, then re-judge census, storage, holds, handles, and owner liveness
under the mutation locks per the re-review's remaining R2/R3/R5 items
in /tmp/rustwrk-2125-rereview.md. The census is inherently O(host
processes); the sequencing must keep that cost OUTSIDE the locks and
bound only the constant-time recheck under them by the tip proof.

# Acceptance Criteria

- Census + handle matching reach Python parity on the re-review's R1/R2
  cases (unrelated process using a row's paths, cgroup-moved owner,
  unreadable `/proc` evidence, foreign/mixed host identity, handle
  naming a row only by resolved path, unreadable handle file, symlinked
  runs directory), each pinned by a test.
- `refuse_apply_census_unavailable` is removed only once every gated
  condition is covered; rows the new checks cannot prove still refuse
  and write nothing.
- #2128's scoped cgroup decision composes with the census exactly as
  in Python: only a no-matches `_CensusDidNotClose` is substituted.

# Notes

Sequel to #2125 wrkslots-history-read-outside-config-lock; filed by
mu-rustwrk at the coordinator's direction (issue number supplied by
coordb).
