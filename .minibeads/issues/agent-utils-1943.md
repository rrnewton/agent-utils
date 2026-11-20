---
title: 'wrkslots-recover-churn-deterministic: grouped-batch completion census rollback and a deterministic real-churn census test are deferred from #1923'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- recover
- census
created_at: 2026-11-20T02:09:27-05:00
updated_at: 2026-11-20T02:09:27-05:00
---

# Description

[watermelon-full-preview] Two deferrals from #1923 wrkslots-recover-late-census-rollback, split out at its cold review so that change stayed the rollback it proved. (1) In a grouped `remove-validate-batch`, when a later slot's fresh per-slot census refuses during completion, that slot is retained still fenced with its finish journal kept and the batch completes the other slots (an existing test asserts exactly that state). Giving that one member a census rollback mid-batch — after siblings may already be removed while the seal is shared — is a batch-level design decision, not part of the single-slot recover rollback. (2) The real-churn recover test (`test_real_churn_late_census_refusal_in_recover_unfences`, six shell loops of short-lived processes against the real same-UID census) depends on host timing: on a lightly loaded host the census scans the churn generations before they exit and closes, so the test failed 4 of 5 cold-review runs with recover succeeding. It carries the `load_quarantine` mark under this issue and is excluded by default.

# Design

- Grouped completion: decide what a census-refused later member leaves behind when siblings are already removed — roll its fence back and keep its finish journal at phase=prepared under the retained seal (the single-slot recover shape), or keep the present retain-fenced shape — and who retires a shared seal that then covers one rolled-back member. Keep the batch report's removed/retained accounting honest in either shape.
- Churn test: make the real did-not-close outcome event-driven (a churn driver whose generations are born and die at census inventory points, in the style of the scripted-host census tests but against real processes), then drop the `load_quarantine` mark. The deterministic real-holder recover tests from #1923 already cover the live-use refusal without timing.
- No census redesign in either half; the census closure rules are unchanged.

# Acceptance Criteria

- Grouped `remove-validate-batch` with a refused later-slot census leaves that slot in the decided state, the batch report names it retained with the census reason, other slots still complete, and a later `wrkslots recover` finishes the refused slot and retires the seal; the previously asserting test is updated to the decided state with its reasoning recorded.
- The real-churn test passes repeatedly (at least 5 consecutive runs) on a host above 75% CPU without the `load_quarantine` mark and without wall-clock budgets (condition-based waits only, owner rule 346), and fails on code where the census has no eight-inventory bound.
- Focused wrkslots suites and a gate pass for each half that lands.

# Notes

Deferred from #1923 wrkslots-recover-late-census-rollback at its cold review (the review judged the recover rollback SAFE and asked for these as follow-ups, not blockers). The `load_quarantine` marker description in py/pyproject.toml names this issue for the churn test until it is made deterministic.
