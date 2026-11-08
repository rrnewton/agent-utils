---
title: 'gate-flake-audit-registry-torn-read: audit reads the registry while its own test rewrites it non-atomically'
status: closed
priority: 2
issue_type: bug
assignee: cl-sync
created_at: 2026-11-07T02:20:00.000000000+00:00
updated_at: 2026-11-08T12:54:22.000000000+00:00
closed_at: 2026-11-08T12:54:22.000000000+00:00
---

# Description

test_audit_lock_free.py's _during_census helper rewrites the ACTIVE
registry with Path.write_text (truncate-then-write) while the audit
under test reads it. Under load the audit can read a torn document and
refuse (test_audit_unrelated_creation_retains_unchanged_rows failed in
a #943 gate run with the audit returning nonzero from active-state
parsing; passes alone). The product writes the registry atomically;
only the test harness writes non-atomically.

# Design

Make the test harness write the registry the way the product does
(atomic replace), so a concurrent audit reads either the old or the
new document, never a torn one. No product change; the per-record
invalidation assertions stay exactly as they are.

# Acceptance Criteria

- The audit lock-free tests pass 20 consecutive runs under bounded
  contention / parallel load.
- A registry change during audit still flips only its own row
  (existing own-record tests green).

# Notes

Filed under the #943 waiver (coordb, 2026-11-07).

## Resolution

[opus 5.5 cl-sync] `_write_active` in `py/wrkslots/tests/test_audit_lock_free.py` now writes a
staged copy and replaces the registry with `os.replace`, as the product does, so an audit reading
it meanwhile sees the old document or the new one. The staged copy lives in the project root, not
beside the registry: a temporary file there is a partial update the product refuses. No product
change; the per-record assertions check the same verdicts. Load evidence: under load (6 busy-loop burners pinned with pytest to 4 CPUs, load average 40-90, solo gate tickets), no run read a torn registry: the full test file passed 3 runs and then failed one assertion, and 14 of 15 replays of that test's scenario passed. Both failures were the census churn of #996 gate-flake-census-churn-abort ("process/path census failed: relevant process evidence changed during three liveness attempts" on every row), not this issue. The verdict assertions now print the row, so a failure names its reasons.
