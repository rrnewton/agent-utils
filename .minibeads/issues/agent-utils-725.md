---
title: 'validate-speed: reduce gate work while preserving observed coverage'
status: closed
priority: 1
issue_type: task
assignee: mu-validate
created_at: 2026-11-01T15:57:04.657241243+00:00
updated_at: 2026-11-01T15:57:04.657241243+00:00
---

# Description

[gpt-6] Measure gate profiles and case timings; remove repeated work; add fail-closed selection within components; make contention-sensitive tests deterministic. Preserve the complete validation DAG contract and record page, Rust-tool, and notes before/after results.

[muse watermelon] Closed by mu-validate: delivered in 4bd9f9e7, 5786f0e6, 8e2c2114, 9d296e04 on main. Final gate: PARTIAL with every selected node passed (215 PASS, 0 FAIL; 7 delegated-live-scope cross skips unverified). See ai_docs/transient/2026-11-01-validate-speed.md.

[muse watermelon] Follow-up under #725 validate-speed (not a new issue): gate admission landed in 1f97c37e (scripts/gate.py, 3 shared slots + exclusive class). The earlier local reference to a separate admission issue number was a mis-allocation and is removed; pushed history is unchanged.

[opus 5.5] 2026-11-02 (lead): gate admission does not span root-isolated slots. scripts/gate.py keeps its admission state in /tmp/dev-agent-utils-gate-admission, but a lane in a root-isolated slot has a private /tmp, so its gates bypass the shared queue and the exclusive class entirely (one host's admission history shows only the cgroup lanes' runs). Fix: an admission directory every box shares (under the harness or project control dir, configurable), or have agentctl/wrkslots bind the admission dir into the box; test that a gate inside an isolated slot queues behind one outside it.

## Governing rule for timeouts (owner policy, 2026-11-02)

[opus 5.5] (1) CPU-time limits are the ones that should bite: set each step's CPU budget at about 1.5x its measured CPU time (50% headroom). (2) Wall-clock limits are very forgiving: about 8x the CPU budget, so contention or a busy host never turns a healthy step red. (3) The pattern of steps timing out under load and passing alone is exactly what this removes. Audit every wall-clock timeout in validate/dagrun steps and the cross differential budgets against it (dagrun already supports CPU budgets). Memory caps are separate: report which steps hit memory caps only under contention.

## Parallelism study (owner, 2026-11-02)

[opus 5.5] "I suspect we are not well tuned for parallelism, and we might eke out a little more, both intra-step and inter-step." Measured study BEFORE any change: (1) inter-step: validate.py caps the graph at VALIDATE_MAX_CPUS=min(32, cpus) and VALIDATE_MAX_STEPS=min(24, cpus), conservative on a 316-CPU host; from dagrun run reports, measure the critical path, CPU utilisation over time and idle gaps for a full --all run and two typical selected runs; (2) intra-step: per step, whether it uses its CPU allocation (cargo test threads, nextest-style partitioning, splitting browser and page shards, the cross differential split into 3); (3) propose new defaults plus re-shards with before/after wall-clock and CPU numbers, under the owner timeout rule (CPU budget ~1.5x measured, wall ~8x). Report as an ai_docs note in the harness repo.
