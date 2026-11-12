---
title: 'mq-validator-purpose-env: pass MQ_PURPOSE to every validator run, local and donor'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-12T16:55:36.321022754+00:00
updated_at: 2026-11-12T21:55:08.357941600+00:00
closed_at: 2026-11-12T21:55:08.357941420+00:00
---

# Description

The validator cannot tell a culprit probe from a batch validation. The local launcher tells it the rerun handle, the base, the parent, fail-fast, peripheral and extra tests, and donor jobs carry only the base, the parent and the extra tests; neither names the job's purpose (batch, probe, widen or remainder). (The admission probe's MQ_PROBE is a different thing.) A consumer adapter therefore cannot give a base probe the exit its evidence deserves: a targeted run can never qualify as a full green, so a probe whose every selected test passed still exits non-zero, and the queue reads it as no evidence (see #1379 mq-unknown-probe-singleton). The adapter half of the fix (mapping the base probe to a qualifying exit) is inert until the queue names the purpose.

# Design

In rs/merge-queue: add PURPOSE_ENV (MQ_PURPOSE) in validate.rs and set it for every validator invocation. Locally, Runner::validate_one adds it from the job's purpose alongside the existing base/parent extras, for both the command and dagrun validators. Culprit-search probes get distinct purposes so a purpose can never be mistaken for landing evidence: the prefix-0 (base) probe is base-probe, the whole-candidate probe is candidate-probe, and interior prefix probes keep probe; batch, widen and remainder keep their names. The queue must never set the base-probe purpose on a validation whose result can become landing evidence: a passing whole-candidate probe can reach the landing path (its pass cancels the red run's failures as flaky), so it carries candidate-probe, never base-probe. A consumer may map only the base probe (where the parent equals the validated commit, so a pass can only clear the base and lead to an ejection, never a landing) to its qualifying exit; every other purpose keeps the unqualified exit. Donor jobs carry the purpose too: add purpose to the published donor job record and bump the donor job schema (host and donors deploy together; a donor that reads a job without a purpose treats it as a batch validation), and the donor worker sets MQ_PURPOSE from the record exactly as the local launcher does. Document MQ_PURPOSE and its values in the validator documentation in validate.rs and the embedded user guide.

# Acceptance Criteria

Every local validator run (batch, base-probe, candidate-probe, probe, widen, remainder) sees MQ_PURPOSE with that exact value, proven by a validator that records its environment. A culprit search over a multi-change batch publishes base-probe only for prefix 0 and candidate-probe only for the whole candidate; a test proves no landing-evidence run (a passing whole-candidate probe that lands as flaky) carries base-probe. A donor job record carries the purpose, the donor validator sees the same MQ_PURPOSE the local launcher would set, and a job record without a purpose still validates as a batch. cargo test -p merge-queue (all targets) passes, including the new cases.

# Notes

Evidence: a 2026-11-12 drain report relayed to this lane. All three base probes in that evidence ran on donors, which is why the purpose must travel in the donor job record and not only in the local launcher's environment. The base-probe/candidate-probe split and the no-landing-evidence condition come from the downstream adapter review (2026-11-12): only the base probe may be mapped to a qualifying exit, because its parent is the validated commit. Until the queue sets MQ_PURPOSE the adapter change is inert and every base probe stays inconclusive. Related: #1379 mq-unknown-probe-singleton (the queue-side handling until then), #1187 mq-singleton-eject (closed; honest assumed-vs-measured base text).
