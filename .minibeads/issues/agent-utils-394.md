---
title: 'ack-delivered-empty: a delivery pass delivered nothing once beside its acknowledgement worker'
status: open
priority: 2
issue_type: bug
created_at: 2026-10-20T10:45:11.891088866+00:00
updated_at: 2026-10-20T10:45:11.891088866+00:00
---

# Description

[muse watermelon] Follow-up from #389 gate-flakes. One observed failure, census-audit coord-gate 2026-10-19: chat_service::tests::the_acknowledgement_worker_adds_the_receipt_reaction_a_printed_prompt_earned panicked with report.delivered == [] under gate load. Not reproduced in #389's investigation (200 debug + 320 release single-test runs pinned to two CPUs with parallel copies, 8 whole-module runs, one full agentctl suite in gate configuration); the assert path contains no sleep or deadline load could stretch, so the likely cause is a load-dependent interleaving of the acknowledgement/receipt workers with the delivery pass over the same request. The assert now prints the whole CycleReport (commit 'agentctl: name the whole delivery report when a pass delivers nothing'), so a recurrence says whether the pass pended, was uncertain, or errored. Diagnose from that evidence; do not widen any wait on speculation. Acceptance: a reproduction (or a recurrence captured with the report) explained and fixed at the interleaving, with the test's assertions unchanged or stronger.
