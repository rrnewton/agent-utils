---
title: 'agentctl-muse-fifo-fixture: pin queue chronology in the cross-language status test'
status: closed
priority: 2
issue_type: task
created_at: 2026-10-22T05:50:20.692024564+00:00
updated_at: 2026-10-22T07:23:06.296395273+00:00
---

# Description

[GPT-6] Published implementation at537f84267eec5d0244e7c5a3aa0dda89d9415953 after full make validate v18 completed exit0 in1783.93seconds at that exact head. Every selected node passed;7alloweddelegated-live-scope crosschecks remain skipped/UNVERIFIED(5cpuset,2DAG). Fresh upstream was unchanged, ordinary linear push succeeded and remote SHA was independently verified. No complete cross-coverage claim. This fixture implementation is closed.


The fixture previously used uncontrolled wall-clock queued_at values and expected arrival order. Production sorts numeric queued_at then filename in both ports. Full validation atb73f5814b624223478a0f51fc640bc233eb48a3f observed2divergences of760pairedchecks: reversed paused/staged order and once-in-order assertion. Discarded queue timestamps were not captured; a wall-clock adjustment is an inference, not measured. The fix pins distinct explicit times and message identifiers opposite lexical arrival order so filename-only or reversed sorting still fails.

Identity correction: our unreleased old412 identifier collided with an upstream idle-TTL feature. Upstream412 was preserved; the fixture record and unpublished commit body were corrected to417 before publication. No shared history rewritten.
