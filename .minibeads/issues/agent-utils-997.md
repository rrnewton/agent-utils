---
title: 'gate-flake-audit-registry-torn-read: audit reads the registry while its own test rewrites it non-atomically'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-07T02:20:00.000000000+00:00
updated_at: 2026-11-07T02:20:00.000000000+00:00
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
