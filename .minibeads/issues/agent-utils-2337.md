---
title: 'wrkslots-plain-remove-validate-complete-uses-teardown-closure: use the validation teardown closure when the recorded owner is proven dead'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- census
created_at: 2026-11-26T20:02:00-05:00
updated_at: 2026-11-26T20:15:00-05:00
---

# Description

Trusted consumer evidence (2026-11-26, 7:42 PM ET): a validate row at generation 1 had a dead recorded owner, but its checkout had been materialized by a dry-run path that wrote no run record. The teardown entry point, `validate-run --retire-unit`, refused ("no run handle ... recorded no checkout to retire"), so the operator fell back to plain `wrkslots remove --validate-complete`. That path ran the general same-UID census and refused under churn, in the same loaded-host conditions as #1883.

Reference acceptance case (same report, relayed 2026-11-26): a generation-1 validate-fresh row whose recorded owner is dead by pidfd, whose recorded owner cgroup is not-found with its cgroup directory absent, and which has no run handle. The launcher's same-launch cleanup had returned not-removed with no error text, and the cleanup-pending row said "checkout removal was not confirmed". `validate-run --retire-unit` therefore skipped with RETIRE-SKIPPED. The subsequent hand removals with `wrkslots remove --validate-complete --census-seconds 600` failed twice with rc 3: "census did not close: 8 rounds, 245 generations", then "same-UID process evidence changed during three liveness attempts" (about 113k tasks, `procs_running` 147-171). This exact shape — dead owner, gone subtree, no run handle — is the fixture this issue must remove through the closure.

The narrow validation teardown closure landed on 2026-11-20 would have covered these facts: recorded owner proven dead, recorded slice subtree empty-or-unknown, and one ordinary all-process reference scan. But the plain `remove --validate-complete` path did not reach that closure for this row, so a case already inside the closure's proof was sent back to the general census that #1883 is about. This issue is only that entry-point gap; it does not ask `validate-run` to invent a missing run record, and it does not decide #1883's general scoped owner/cgroup census direction.

# Design

- In plain `remove --validate-complete` for a validate slot, when the recorded owner is proven dead, take the same validation teardown closure (B2-B) instead of the general following census: recorded owner proven dead (pidfd), recorded slice subtree empty-or-unknown, and one ordinary all-process reference scan finding no holder. Reuse the existing closure rather than restating its conditions in a second implementation. Do not copy the existing private-cleanup call sites' `slice state != "unknown"` selection gate blindly: the reference case's recorded cgroup was not-found and its directory absent, so this entry point must let the closure's own unknown handling apply — an absent/unknown subtree is not affirmative emptiness evidence, and the one reference scan decides alone.
- The closure's safety argument lives in `py/wrkslots/cli.py` in the docstring of `_assert_validate_teardown_unused` ("Validation-slot teardown closure (B2-B)"), exercised by `py/wrkslots/tests/test_validate_teardown_closure.py` and `py/wrkslots/tests/test_slice_fallbacks.py`: the subtree check cannot see a box launched without the slice, so a holder outside it is decided by the same single reference scan, while an absent/unreadable slice proves nothing either way. Extending the closure to this entry point needs the same independent safety review as the original closure change: reviewer checks the owner-death proof is fresh at removal, the fence/identity and deletion ordering are the same as the paths that already use the closure, the unknown-slice case still lets the reference scan decide alone, and any positive holder still refuses without deletion.
- Preconditions stay refuse-closed: no recorded owner generation, an owner not proven dead, a nonempty recorded slice subtree, or a holder found by the reference scan keeps the existing refusal/general-census behaviour for that shape. Live-owner validate finishes and non-validate slots are unchanged.
- Keep the removal's existing evidence budget, finish fence, and recheck-before-deletion discipline; do not start destructive work after the applicable evidence deadline. If the plain-remove proof cannot be placed under the same budget/lock discipline as the existing call sites, say so in the Resolution rather than widening the change.
- Both editions where the surface exists. The same-UID following census is Python-only today per #1883, but confirm whether the Rust remove/validate-complete path needs the same entry-point change or records why it does not.

# Acceptance Criteria

- The reference acceptance case above removes through the closure: a generation-1 validate-fresh row with its recorded owner dead by pidfd, its recorded cgroup not-found and directory absent (slice state unknown), no run handle (so `validate-run --retire-unit` returns RETIRE-SKIPPED), and a cleanup-pending row saying "checkout removal was not confirmed" is removed by plain `remove --validate-complete` when the one reference scan finds no holder; it does not need the general same-UID census to close under unrelated churn.
- The refusal shapes stay closed: live or indeterminate owner, no recorded owner generation, nonempty slice subtree, and a sibling-scope holder found by the reference scan all refuse with nothing deleted.
- Existing teardown-closure, slice-fallback, validate-complete removal, and census suites pass, in both editions where the surface exists (or the Resolution records the parity answer).
- The change carries an independent safety review of the same standard as the original B2-B closure, recorded in the landing/Resolution.

# Notes

Split from #1883 on 2026-11-26 at the coordinator's direction, from the same trusted consumer report as the #1883 addendum of that date. #1883 remains the general-census problem (sustained churn of new same-UID generations; scoped owner/cgroup direction awaiting the owner's decision). This issue is deliberately narrower: it only makes an already-reviewed closure reachable from the plain removal entry point for the proof shape it already covers.
