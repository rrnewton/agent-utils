---
title: 'validation-adapter-executable-cache: the Rust adapter executables the Python differentials compare against come from a content-keyed host cache when their inputs are unchanged'
status: open
priority: 2
issue_type: feature
assignee: cl-shardcap
labels:
- validation
- python
- rust
created_at: 2026-11-26T20:45:00-05:00
updated_at: 2026-11-26T20:45:00-05:00
---

# Description

Three test helpers release-build a Rust adapter executable for the Python differentials, each into its own private target under `rs/target/validation-adapters/`:

- `py/tests/rust_binary.py` builds agentctl and the fake chat plugin.
- `py/wrkslots/tests/rust_binary.py` builds wrkslotsd.
- the wrkviz helper (#2318) builds wrkvizd.

#2306 and #2318 run each build in its own DAG step before the shards, so the compile is no longer charged to a shard's memory cap. That moves the compile; it doesn't remove it. Gates usually find these targets cold or stale, because every gate runs in a different worktree and on a different tree:

- From 2026-11-20 to 2026-11-26, all 97 agentctl shard runs compiled agentctl.
- In 57 of 84 gate-like runs since 2026-11-20, one wrkslots shard has a compile-sized peak.

Measured costs on 2026-11-26 (two Cargo jobs, load1 165-199):

| Build | Wall | CPU | Peak |
|---|---|---|---|
| agentctl, empty target | 299 s | 528 CPU-s | 2.76 GiB |
| agentctl, dependencies warm, crate changed | 226 s | | 2.39 GiB |
| agentctl, warm | 0.6 s | 0.3 CPU-s | |
| wrkslotsd, cold | 161 s | 282 CPU-s | 1.43 GiB |
| wrkviz, cold | 167 s | 304 CPU-s | 1.35 GiB |

Most of that time is the package's own crate: the agentctl rustc alone ran 218 s. A host-shared Cargo target would not help, because Cargo keys a workspace crate's artifacts by its source path, so the crate recompiles in every worktree; only the dependencies would be reused.

Many gates build an executable whose inputs are byte-identical to one already built on the host: a whole-suite gate of a change that doesn't touch the crate, or a rerun. Those builds can be skipped entirely.

# Design

- **Key.** Each helper computes a key from everything that determines the executable:
  - the git tree id of every workspace crate the package depends on (from `cargo metadata`, the package itself included);
  - the content hash of `rs/Cargo.lock` and of the workspace `rs/Cargo.toml`;
  - `rustc -vV`;
  - the package, the binaries and the build flags and environment the helper uses (release, `--locked`, the profile variables `rust_agentctl_with_fake` sets).
- **No key for a dirty tree.** If any of those paths has uncommitted changes (tracked or untracked, by `git status --porcelain` over them), the helper computes no key and builds exactly as today.
- **Hit.** If `~/.cache/agent-utils/validation-adapters/<package>/<key>/` holds a complete entry, the helper returns the cached executables without running Cargo. A complete entry has every executable plus a manifest naming the key inputs, written last. The entry is used read-only: executed in place, never written, never moved.
- **Miss.** The helper builds into its private target as today, then publishes:
  - copy the executables and the manifest into a temporary directory beside the entry;
  - `rename` it into place;
  - if another process published the same key first, its entry is kept and the temporary one is removed.
  A failed build publishes nothing.
- **Pruning.** It runs at publish time, and only while no gate ticket is queued or running on the host (gate.py's queue reports none), so no entry is ever removed under a ticket. It removes:
  - entries older than a configured age;
  - entries beyond a configured count per package;
  - the oldest entries until the cache is under a configured size. The size cap keeps the cache inside the footprint and disk floor rules.
  A process that loses a race with pruning falls back to building.
- **Opt-out.** An environment variable disables the cache, for measurement runs that must build.
- No change to the DAG steps: `python.<component>.build-rust` keeps running the helper, which now often returns at once. No change to the tests' comparisons.

# Acceptance Criteria

- Test: a clean tree whose adapter inputs match a cached entry returns the cached executables without running Cargo.
- Test: changing a file in the package's crate (or in a workspace crate it depends on) changes the key, so the cache is missed and the helper builds.
- Test: a dirty tree computes no key and always builds, and nothing it builds is published.
- Test: publishing is atomic (an incomplete entry is never returned) and two processes publishing the same key leave one complete entry.
- Test: pruning removes nothing while a gate ticket is queued or running, and otherwise enforces the age, count and size limits.
- Measured: in a gate whose tree matches an earlier build on the host, each `python.*.build-rust` step completes without running Cargo, recorded in this issue with its time and CPU.
- No wall-clock pass/fail budget is introduced (owner rule 346).

# Notes

Proposed while measuring the build steps' cost for #2306 and #2318, and filed at the lead coordinator's request. Do it after #2318 lands, as its own commit and gate. Related: #2306 (`python.agentctl.build-rust`), #2318 (the wrkslots and wrkviz build steps and the wrkviz helper), #883 (two Cargo jobs for test-shard builds), #818 (memory hint rule).
