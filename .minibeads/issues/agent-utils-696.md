---
title: 'wrkslots-shard-timeout: wrkslots Python shards run ~596 s against a 600 s cap'
status: open
priority: 2
issue_type: task
created_at: 2026-10-29T16:38:12.555586966+00:00
updated_at: 2026-10-29T16:38:12.555586966+00:00
---

# Description

[watermelon-full-preview] The wrkslots Python component tests are sharded in `validation/python-components.dag.yaml` as `python.wrkslots.shard-0` (`--shard 0/2`) and `python.wrkslots.shard-1` (`--shard 1/2`), each with a 600 s node timeout (`timeout: 600`, `cpu_timeout: 1200`, `est_duration_s: 330`). The DAG comment records: "Two shards: one process measured 626 s on 2026-10-15 (763 tests), past the 600 s bound, once the ownerless-clone suite was added."

Measured 2026-10-29 during #680 launcher-fast-path gating: standalone `python3 scripts/run_component_tests.py --component wrkslots --shard 0/2 -- -q` passes in 596.72 s (594 passed) on an otherwise loaded host — about 3 s under the cap. Inside the flattened validation DAG, where the shard shares the host with the lifecycle namespace shards, Rust test suites, and browser checks, the same shard exceeded 600 s in seven consecutive gate runs (2026-10-29), each time as the sole failing node (the other shard then ABORTs on eager exit, and about 60 downstream browser checks are not launched). Gate run wall time is capped near 603 s, so one marginal shard blocks the whole gate.

This makes the portable validation gate effectively unpassable whenever the host is busy, for any change, including changes that touch no wrkslots code. Options to consider: raise the node timeout, split into more shards, reduce shard work, or make the shard faster. Until then, landing follows the per-step rule: every selected step must pass in some run on the final commit, with the shard passing standalone.

### Fix landed

- Commit: `e25d7cbe`.
- `python.wrkslots.shard` splits into four shards (`0/4`..`3/4`) in `validation/python-components.dag.yaml`, with the step cap set from measured times: 1500 s timeout / 2400 s cpu_timeout, `est_duration_s: 520`.
- `scripts/run_pytest_shard.py` now balances families by recorded duration: `_assign` weights a family by its recorded seconds from `scripts/pytest_shard_weights.json`, falls back to case count scaled by the median recorded seconds-per-case for families the file does not know, and is exactly the old count balancing when no weights exist. A present-but-malformed weights file raises rather than silently unbalancing; non-finite durations are rejected.
- `scripts/pytest_shard_weights.json` is generated from recorded per-case timings by the new `scripts/case_timings.py weights` subcommand (verified timing directory only; nested reruns excluded): 815 families, 1,287 cases, 1,827.5 recorded seconds from the two solo halves.
- Why: the count-balanced halves measured 836 s and 993 s solo on 2026-10-30 (simulation from the same timings: 835/992), past the 600 s cap, so every gate in every lane timed out on them. Duration-weighted quarters measured 482/463/447/554 s solo on the landing tree (1,310 cases total) and 830-898 s inside the full gate, which sizes the 1500 s cap. `lifecycle-io` rises 11 to 13 for the two added component shards (9 lifecycle + 4 component).
- Gate 107: 206 passed; the only failure was `repository.hygiene.validation-selector` (the new weights file was unclassified), fixed by classifying it in `validation/components.json` and re-passed solo. All four new shards passed inside the gate.
- Adversarial review was attempted three times; each reviewer subagent died with a runtime error, so the author self-reviewed completeness, determinism, loader edges and other components' behaviour before landing.
