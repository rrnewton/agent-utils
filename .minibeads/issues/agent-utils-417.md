---
title: 'agentctl-muse-fifo-fixture: pin queue chronology in the cross-language status test'
status: closed
priority: 2
issue_type: task
created_at: 2026-10-22T05:50:20.692024564+00:00
updated_at: 2026-10-22T06:21:38.344493072+00:00
---

# Description

[GPT-6] Current state: deterministic Muse queue chronology and reverse-lexical identifiers are implemented; publication remains pending. Full make validate completed exit0 on27979dff6867ab37ebffb1c816e632d438a73262 in1838.22seconds. Every selected node passed;7alloweddelegated-live-scope crosschecks remain skipped/UNVERIFIED(5cpuset,2DAG), not complete cross coverage. The focused two-edition fixture passes23checks without host CLI calls. Both real CLI enqueue/hold/drain and exact-once ordering assertions remain intact.

The fixture previously used uncontrolled wall-clock queued_at values and expected arrival order. Production sorts numeric queued_at then filename in both ports. Full validation atb73f5814b624223478a0f51fc640bc233eb48a3f observed2divergences of760pairedchecks: reversed paused/staged order and once-in-order assertion. Discarded queue timestamps were not captured; a wall-clock adjustment is an inference, not measured. The fix pins distinct explicit times and message identifiers opposite lexical arrival order so filename-only or reversed sorting still fails.

Fresh post-validation upstream commits throughc8655de7 changed the message-service open-link feature (runtime web assets and other component source); ordinary rebase succeeded. Current rebased head still requires required validation before ordinary linear publication. No current-head fullgreen or publication claim; no weakened selection/expectation. Use8actualCPU throughout,20GiBnoSwap,serial,oneCargo4buildjobs,journal-onlyRAM/normalTMPDIR/realprivateTZDIR. Do not dynamically reduce CPU below fixture requirements.

Identity correction: our unreleased old agent-utils-412 identifier collided with an upstream idle-TTL feature. Upstream412 is preserved unchanged; this implementation is agent-utils-417. Earlier unpublished commit text is corrected to417 before publication; no shared history rewritten.
