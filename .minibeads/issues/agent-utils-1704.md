---
title: 'validation-deps-check-memory-cap: dagrun deps checks OOM under their learned memory cap'
status: open
priority: 1
issue_type: bug
labels:
- validation
created_at: 2026-11-16T06:20:00.000000000+00:00
updated_at: 2026-11-16T06:20:00.000000000+00:00
---

# Description

The dagrun dependency checks (`rust.dagrun.testrun-core-deps`,
`rust.dagrun.dagrun-model-deps`) run `cargo tree` / `cargo metadata`
under a learned inner memory cap of 73,924,608 bytes (70.5 MiB). A good
run peaks at about 56-57 MiB RSS, only ~15 MiB under the cap, so under
gate concurrency the same step OOM-kills at exactly the cap. Observed
2026-11-15/16: the step failed in gates on an unrelated change with no
Cargo.toml/Cargo.lock diff and a closure (serde, serde_json, libc,
itoa, memchr, zmij) that excludes the changed crates; capped direct
dagrun runs pass on both main (6218bb40) and the change tree, and the
plain script passes solo. History: these steps have failed in 8 gates
since 2026-11-11. Owner rule 346: a marginal timing/memory cap must not
gate unrelated work.

# Design

Give both jobs an explicit `hard_mem_max_bytes: 268435456` (256 MiB) in
`validation/rust.dag.yaml`, with a comment naming this issue. Do not
disable the checks; the dependency rules themselves are unchanged.

# Acceptance Criteria

- Capped dagrun runs of both selected steps pass repeatedly, including
  through gate.py.
- No other job's cap changes.

# Notes

Comparison numbers from the investigation: cap 70.5 MiB, passing peaks
~56-57 MiB, OOM rows at exactly the cap, direct capped runs green on
main and on the change tree, failures only through gate concurrency.
