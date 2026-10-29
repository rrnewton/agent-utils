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
