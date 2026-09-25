# Validation overhaul audit

Date: 2026-09-25 UTC

This is working evidence for the validation-overhaul project, not settled user-facing
documentation. It records the original timing census, the implemented validation architecture,
the measured concurrency choices, and a completed under-ten-minute full-validation run on the
corrected current tree.

## Evidence boundary

The baseline was launched at Git commit
`d893100730bb3a7f115724431c6c619be8164dfe`. The launch-time worktree status was not captured in a
machine-readable artifact, so it must be treated as unproven rather than asserted clean. The three
baseline JUnit files nevertheless contain exactly 5,734 distinct tests, matching the collected
suite: 4,604 general tests, 848 lifecycle tests in a mapped user/PID namespace, and 282 lifecycle
tests in the ordinary host environment. Their sets are disjoint and their union is complete.

The checkout was dirty when this report was generated because the validation overhaul was already
being implemented. Corrective reruns are therefore recorded separately from the baseline; they are
not presented as one end-to-end clean run. Exact file hashes, correction rules, and completeness
checks are in `test-timing-provenance.json`.

JUnit `time` is elapsed test-case time, including fixture work. It is not CPU time. Dagrun step
profiles must supply user CPU, system CPU, memory, throttling, and wall time for the parallelism
sweep. The one-sample baseline is a scheduling seed, not a stable performance claim.

## Implemented architecture

### One graph, composed at load time

`validation.dag.yaml` is now the single portable, current-toolchain repository contract. It
includes ten focused fragments --
repository checks, ordinary Python components, Python lifecycle tests, Rust tests, cross-language
tests, examples, Python packages, Rust packages, timeline/browser tests, and vibe-talk -- and one
outer dagrun invocation plans the resulting graph. Shared resource caps therefore apply across
languages and tools, and independent work can overlap instead of waiting for a Python phase, a
Rust phase, and separate package scripts to run serially. Selected validation and full validation
both load this same graph; selection changes labels, not the definition of a check.

DAG inclusion is strict compile-time composition, not a runtime subgraph node. Each included
fragment receives a namespace that prefixes its steps and internal references. Nested namespaces
compose with dots. A parent's optional `after` references are resolved in the parent's namespace
and injected into every entry node of the included flattened subgraph. In the other direction, a
parent connects to work inside a fragment by naming an exact flattened step tag. There is no
synthetic include node and no implicit whole-fragment completion barrier: the parent owns boundary
wiring, while the child remains reusable.

The loader fails before planning on malformed or unknown include fields, missing or escaping
paths, namespace collisions, cycles, duplicate effective includes, policy conflicts, invalid
`after` targets, or expansion-limit violations. Includes are refused entirely by path-free parsing
and standard input because those inputs have no safe directory against which to resolve a path.
The exact `include` key is reserved, so composition can never be silently ignored by an older or
context-free loader.

### Fail-closed selection and ownership

`scripts/validate.py` gathers committed, unstaged, and untracked paths. Rename detection is
deliberately disabled so both the removed and added paths contribute to the selection. Broad path
rules select validation groups, while `validation/components.json` maps tool source prefixes to
component checks and records reverse dependencies. The checked-in manifest currently owns 191
ordinary Python test files across nine components and classifies the lifecycle file separately.
Its self-test fails on every unclassified, missing, or multiply owned Python test file.

An unknown path selects the complete portable contract, and a change spanning areas selects the union of
their requirements. Repository-wide hygiene remains selected for every change. A component-only
edit selects only that tool's labelled Python, Rust, cross-language, and packaging nodes plus its
declared reverse dependencies; `--all` omits label filtering and executes the entire flattened
graph. This makes selection a latency optimization rather than a second, weaker test contract.

### One owner for nested execution

Most graph steps are ordinary child processes of the one outer scheduler. The few tests that
intentionally start dagrun themselves must opt in with the typed `delegated_children: true`
property. When cgroup boxing is available, the outer run retains ownership of the complete subtree,
puts the command in a supervisor leaf, and delegates an empty step root to the inner scheduler.
Cancellation therefore still reaches descendants that escape a process group. An explicitly
unboxed outer run may pass a narrowly scoped fallback marker only to an opted-in step; ordinary
children do not inherit it, malformed markers and forged cgroup paths fail, and the fallback warns
that it is using process-group/procfs accounting rather than claiming cgroup containment.

Python test shards preserve parameterized families, use independent pytest temp roots, and keep
the destructive lifecycle suite in its own namespace/host-visible lanes. Cargo consumers share a
repository-wide target-directory cap. The default validation driver gives the outer critical-path
planner at most 24 simultaneous steps and 32 CPUs; per-resource caps and measured inner widths
further constrain the work that can actually overlap.

## Baseline result

| Phase | Cases | Result | Suite wall | Sum of cases | Share of case time |
|---|---:|---|---:|---:|---:|
| General | 4,604 | 4,589 pass, 15 fail | 733.080 s | 728.721 s | 26.5% |
| Lifecycle, namespaced | 848 | pass | 1,355.402 s | 1,354.718 s | 49.2% |
| Lifecycle, host-visible | 282 | pass | 669.604 s | 669.274 s | 24.3% |
| Total | 5,734 | baseline not green | 2,758.086 s | 2,752.713 s | 100% |

The sequential Python phases alone occupied about 46 minutes. Cost is concentrated: 182 cases
accounted for 50% of raw case time, 574 for 80%, 836 for 90%, and 1,088 for 95%.

## Corrections and findings

The complete TSV keeps the raw baseline value and a separate effective value for every test. The
effective values substitute measured reruns only where the baseline was invalidated by a known
environment or harness problem.

1. Thirteen engine-resolver tests failed immediately because the measurement command's `PATH`
   omitted `cargo` (twelve tests) or `rustc` (one test). With the toolchain on `PATH`, all thirteen
   passed in 22.608 seconds of JUnit suite time. These are environment failures, not product
   regressions, and the original near-zero failure durations understate a clean run.
2. Two process-readiness tests failed at their readiness deadlines under load. They passed together
   in 2.970 seconds, then passed in ten further invocations (20/20 cases). The ranking uses the
   eleven-rerun median for each case. This is load-sensitive readiness flakiness, not a reproduced
   functional regression.
3. The agent-log archive module cost 199.585 seconds because ordinary fetch tests repeatedly probed
   one ambient stale tmux SSH-agent socket through its real ten-second ceiling. Isolating ambient
   tmux from those generic subprocess tests, while adding a direct live-tmux-agent test, produced
   33/33 passing tests in 10.991 seconds; the 32 baseline cases contributed 10.838 seconds. This is
   harness overhead, not useful regression coverage.
4. One host-visible lifecycle race cost 168.790 seconds because each semantic checkpoint scanned
   every unrelated process on the shared host. It now enumerates a tiny synthetic proc root whose
   sole entry is a symlink to the late entrant's real `/proc/<pid>` directory. The entrant is still
   created after the second scan, and the production scanner still consumes real kernel evidence.
   Three post-change JUnit reruns passed at 4.239, 4.373, and 4.899 seconds. The median is 4.373
   seconds, a 97% reduction. Thirteen neighboring real lsof/process-census cases also passed.
5. Profile feedback learned quiet-run footprints for the two Python dagrun shards and reduced
   their effective caps to 548.4 MiB and 578.7 MiB. Under full-suite concurrency each shard in
   turn reached that exact cap and recorded `memory.events: oom=3`. The delegated-root view had
   correctly excluded descendant-owned kills, but the terminal diagnostic looked only at
   `oom_kill=0` and misleadingly called the resulting SIGKILL unexplained. These variable,
   nested-process workloads now carry explicit 2 GiB hard caps. Both engines classify positive
   `oom`, `oom_kill`, or `oom_group_kill` evidence as a memory-cap failure and persist the group
   counter; 905/905 shard cases then passed together with profile feedback enabled.
6. Two full-load-only harness races were made explicit. The cpuset interoperability check now
   retries the complete two-engine attempt, with a fresh ledger, only for the exact transient
   systemd user-bus `Connection refused` signature; a deterministically injected refusal retried
   once and then passed all 58 checks, while 12 delegated stress copies passed. An agentctl
   process-containment fixture now atomically publishes its two-PID marker and waits for a complete,
   parseable record rather than treating file creation as payload readiness; its three cleanup
   modes passed 120/120 concurrent stress cases.
7. A post-run profile audit found three fresh-profile-store ceilings with insufficient evidence or
   headroom. One lifecycle shard had later reached its explicit 768 MiB ceiling with 845 reclaim
   events, so that hard cap was removed in favor of the authored 1 GiB baseline and its 1.25 runtime
   margin. The component-manifest floor was raised from 128 to 160 MiB after a 150.55 MiB successful
   peak, and the agentctl differential floor was raised from 2048 to 2560 MiB after a 2229.57 MiB
   successful peak. These changes prevent a fresh profile store from receiving tighter caps than
   the measured workload supports.
8. Full-graph concurrency exposed two publication races that focused runs had not. The wrkslots
   stress fixture could expose an empty or partial JSON control request while writing it in place;
   it now publishes with a temporary file and atomic rename, and four simultaneous end-to-end
   reruns passed. In both dagrun engines, an injected profile-write crash could release accounting
   and remove a tag from `running` before its terminal result reached `done`, allowing a duplicate
   launch that never retired. Terminal publication now retires accounting, records `done`, and
   only then removes `running`; the exact Python race passed 20 repeated runs, alongside the full
   Python scheduler and Rust dagrun suites.
9. Two memory-feedback tests still encoded the superseded rule that ordinary observations could
   lower an authored RSS hint. They now distinguish the safe default, which preserves the authored
   lower bound, from explicit `--profile-memory-feedback`, which may use an adequately sampled
   lower observation. The focused cross-runtime memory contract passed all 11 checks.

After those measured substitutions, the 5,734 baseline cases represent 2,410.090 seconds of
sequential case time. This is a cross-run planning estimate, not a replacement for a green full run.

## Highest corrected costs

| Seconds | Phase | Test | Assessment |
|---:|---|---|---|
| 39.884 | general | `wrkslots/tests/test_e2e_stress.py::test_real_process_and_git_invariants` | Keep: real process/Git end-to-end safety; isolate and profile its internal phases. |
| 22.040 | host lifecycle | `test_remove_refuses_live_process_using_slot` | Keep as one real host/lsof positive anchor; do not multiply it across equivalent matrices. |
| 20.811 | general | `tests/test_cli.py::test_unboxed_run_enforces_a_lower_bound_and_exposes_its_escape` | Keep: direct regression anchor for CPU enforcement and its escape boundary. |
| 17.458 | namespace lifecycle | `test_clean_agent_remove_with_recursive_submodules_preserves_peers_and_config` | Keep: destructive recursive-submodule integration; shard. |
| 17.410 | namespace lifecycle | `test_audit_reports_deletable_blocked_held_and_the_leak_invariant` | Keep: lifecycle classification and leak invariant; shard. |
| 14.453 | general | `test_subject_cursor_is_fair_with_different_root_counts_and_oversized_peer` | Preserve fairness observations; stop once the stated observations are satisfied if the long-tail cycles add no distinct assertion. |
| 13.909 | general | `tests/test_cli.py::test_default_small_cpu_cap_is_enforced_and_allows_compliant_work` | Keep: real default-cap enforcement anchor. |
| 13.681 | general | `test_default_budget_large_root_has_finite_explicit_terminal_outcome` | Separate the production-constant assertion from a smaller behavioral boundary where possible. |
| 10.090 | general | `test_default_budget_blocked_traversal_terminates_on_every_attempt_and_retries` | Keep kill/reap semantics but use a short explicit test deadline; a neighboring 0.2-second case already proves the same worker boundary. |

The namespace lifecycle cost is broad rather than dominated by a single wait: 120 of 848 cases
make up 50%, 311 make up 80%, and 471 make up 95%. Those tests exercise destructive Git,
submodule, crash-recovery, fail-closed parsing, and process-liveness behavior. Their first treatment
should be isolation-preserving parallelism and targeted selection, not deletion.

The host suite contains large setup matrices. In particular, the families checking pristine
recursive checkouts, fresh-census rechecks, and exact current shapes account for roughly 202
seconds. They cover distinct fail-closed states, but each case rebuilds substantial repository
state. Reduce their total CPU only after either sharing an immutable setup safely or refactoring
classify/recover through a common seam; do not prune one endpoint while those paths remain separate.

## Ranking artifact

`test-timing-ranking.tsv` is sorted by corrected/effective seconds descending and contains every
baseline node ID exactly once. The columns are:

- `rank`, `phase`, `isolation`, and `component`;
- exact `nodeid` and parameter-family identity;
- baseline and effective status/time;
- effective share and cumulative share of all 5,734 cases;
- timing source, coverage-contract category, and recommendation category.

For the legacy xunit2 files, exact node IDs were reconstructed by choosing the longest prefix of
`classname` whose dot-to-slash translation names an existing Python test file, appending any
remaining class components with `::`, and then appending `name`. All 5,734 reconstructed IDs matched
the pytest collection cache with zero misses and zero collisions. Future runs should put `nodeid`
and family directly into JUnit `properties`; reconstruction should remain only a compatibility path.

Coverage and recommendation values are family-level audit categories, not line-coverage claims.
Unknown tests must fail the report's classification check rather than silently receiving a cheap
default. Branch coverage and mutation evidence may refine these categories later, but neither can
replace review of destructive safety invariants.

The ranking is a power-to-weight review tool, not an instruction to delete the slowest tests. The
highest-cost cases disproportionately cover destructive Git behavior, process liveness, cgroup
enforcement, crash recovery, and fail-closed parsing. The implemented changes first removed
ambient-host probes and repeated setup that added no assertion, then isolated and sharded the
remaining safety coverage. A test should be removed or weakened only when a cheaper test proves the
same invariant; raw duration alone is not evidence that coverage lacks value.

## Full-graph gate ranking

`gate-timing-ranking.tsv` complements the Python case ranking with every one of the 78 executable
gates in the flattened validation graph. It ranks their measured wall and aggregate CPU time,
records peak memory and applied cap, and gives each gate a coverage rationale and recommendation.
This is the correct resource-planning unit for Cargo, cross-language, packaging, browser, example,
and application suites, whose nested frameworks do not currently emit one common per-case timing
schema. The existing 5,734-row TSV retains the finer Python-case view.

The checked-in gate ranking describes green run `18d885f0608c7bd9002d2cf9`: 371.650
seconds of graph wall time, 3,272.204 aggregate step-wall seconds, and 2,916.240 aggregate CPU
seconds. `gate-timing-provenance.json` binds the report to the exact 78 profile rows and canonical
gate listing, and hashes the public `dagrun json` expansion so commands and resource limits from
every included fragment are covered too. `build_gate_timing_ranking.py` rejects missing, blank, or
duplicate proof fields, inconsistent run metadata, any failed, timed-out, or OOM-observed row, and
any mismatch with the graph loaded by the public `dagrun list` command. It publishes the TSV and
provenance through fsynced temporary files, with the self-hashing provenance replaced last.

The ranking also makes the remaining wall-time opportunity explicit. The dagrun cross-language
differential occupied 368.428 of the graph's 371.650 seconds (99.1% of the critical-path wall) while
using 386.617 CPU-seconds, or about 1.05 effective cores. The whole graph averaged 7.85 measured CPU
cores against its 32-CPU ceiling. Outer-DAG concurrency is therefore no longer the limiting factor:
materially shortening the full run now requires safely splitting or parallelizing that differential
gate. The under-ten-minute target is demonstrated, but maximal machine utilization is not.

## Measured concurrency choices

### Lifecycle outer width

Each lifecycle trial ran all 1,130 expected cases exactly once and passed. The reported CPU value
is total user plus system CPU across the run.

| Concurrent lifecycle nodes | Wall | CPU total |
|---:|---:|---:|
| 4 | 247.7 s | 730.9 CPU-s |
| 5 | 190.0 s | 743.0 CPU-s |
| 6 | 218.3 s | 748.9 CPU-s |

Width five is retained as the `lifecycle-io` cap. It was the fastest observed run and used less CPU
than width six. The host-visible subset has an additional cap of three because those tests share
the host process namespace; namespaced shards each receive their own user/PID namespace and temp
root.

### Cargo inner width

The Cargo sweep used a dedicated target directory and cleaned it before every trial, so every row
is a cold full-workspace release build rather than an incremental successor. This is one measured
trial per width, not a claim about low-variance medians.

| Cargo jobs | Wall | CPU total |
|---:|---:|---:|
| 1 | 263.884 s | 255.925 CPU-s |
| 2 | 130.638 s | 258.583 CPU-s |
| 4 | 69.552 s | 260.103 CPU-s |
| 8 | 43.230 s | 258.705 CPU-s |
| 16 | 41.527 s | 261.068 CPU-s |
| 24 | 45.097 s | 263.881 CPU-s |
| 32 | 41.591 s | 269.251 CPU-s |

Eight jobs is the chosen elbow. It is the smallest width within 5% of the fastest observation
(43.230 seconds versus 41.527 at width 16), while higher widths provide no stable wall-time gain
and width 32 consumes more CPU. The workspace build and its incremental clippy successor therefore
request eight inner jobs. Pytest remains parallelized as outer DAG shards rather than pretending a
larger inner-width hint can parallelize one sequential pytest process.

## End-to-end evidence

A documentation-only selected run executed its seven always-on hygiene nodes successfully. The
flattened graph reported 1.0 seconds wall time, and the complete selector/driver invocation took
1.5 seconds. This establishes that prose-only changes no longer pay for unrelated language,
package, browser, or application suites.

The bare `make validate` proof (`run_id=18d885f0608c7bd9002d2cf9`) executed all 78 nodes
exactly once and passed with zero failures, aborts, skips, or unlaunched work in 371.7 seconds
(6 minutes 11.7 seconds). Its per-step profile records 2,916.240 aggregate user+system CPU-seconds,
wall offsets, peak memory, applied memory caps, throttling, pressure, and OOM counters. The longest
node was the 798-check dagrun cross-language differential at 368.4 seconds; independent work
overlapped it rather than forming serial language phases. No validator, pytest-shard, or dagrun
process remained afterward, and the successful run created no persistent pytest temp root.

A representative synthetic change to `py/tick_hub/cli.py`, passed through the real selection
function and the same graph loader (`run_id=18d88315e12d6b26003e9226`), selected 17 nodes and passed
in 13.9 seconds of graph time / 14.76 seconds end to end, consuming 42.246 aggregate CPU-seconds.
Together with the 1.5-second documentation-only proof, this demonstrates that routine changes no
longer pay for unrelated tools while the full portable contract remains a single explicit command and stays
below its 600-second target.

This proof includes the resource-model, cap, harness-race, and terminal-publication corrections
described above. The generated gate ranking and provenance were rebuilt directly from its 78 green
profile rows.
