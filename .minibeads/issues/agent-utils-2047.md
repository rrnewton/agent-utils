---
title: 'wrkslots-missing-sidecar-red-control: the missing-sidecar regression test could not fail on a broken compare'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T10:15:00-05:00
updated_at: 2026-11-21T10:15:00-05:00
---

# Description

[watermelon-full-preview] The deferred-remove sidecar regression test
(`test_private_finish_compare_fails_closed_without_sidecar`) passed even under a
mutation that made every finish journal compare equal: journal loading
validates the shape first, and that validation expands the missing sidecar and
refuses before the compare is reached. So the test proved nothing about the
canonical compare `_finish_journals_equal` — a compare that always answered
`True`, or an expansion that treated a missing sidecar as nothing, would have
left it green.

# Design

Strengthened the test to call the canonical compare directly on the shrunk
on-disk journal dict after unlinking the sidecar file, and to assert that
this call refuses with `StateError` naming the sidecar. Only then does it
exercise the public reload (`_load_prepared_private_finish`) as before.

# Acceptance Criteria

- The strengthened test passes with the production compare.
- The strengthened test fails when `_finish_journals_equal` is stubbed to
  always answer equal.
- The strengthened test fails when `_expand_containing_refs_sidecar` is
  stubbed to swallow a missing file and treat the inventory as absent.
- No wall-clock budgets: the refusals are all deterministic file/state
  outcomes.

# Notes

Follow-up to #2041 wrkslots-deferred-finish-journal-race; the sidecarred
inventories themselves are #1757.
