# test-time-agent-utils: the suites outside vibe-talk, measured and cut

[opus 5.5] Findings of the 2026-10-24 test-suite optimization round for everything outside
vibe-talk (#494 test-time-agent-utils): wrkslots, agentctl, dagrun, herdr-run, tick-hub,
pr-landing-planner, wrkviz, packaging, site, the cross/ checks and the Python shards. vibe-talk is
the other lane (`2026-10-24-test-time-vibe-talk.md`). The owner's brief: anything over ten
minutes deserves inspection, and pruning where its signal is redundant or out of proportion to
its time.

All numbers are from the validation host (316 CPUs, shared with other agents' gates, load
average 33-64 during this work; each gate runs under a 32-CPU per-run cap and `--max-steps 24`).

- **Before:** a full-graph run of `python3 scripts/validate.py --all` on `285c0885`, with its
  per-case timing records kept: **830.3 s wall, 11 978 s CPU** (174 steps).
- **After:** the landing gate on the final tree (`validate.py --base origin/main`, which
  selected the whole graph). **612.0 s wall, 8 090 s CPU**
  (179 steps; rc 0, `validate: PARTIAL` only for the 7 standing delegated-live-scope dagrun cross
  skips that every full-graph gate on this host shows). The gate is shorter for two reasons: this
  lane's changes, and the vibe-talk lane's, which landed in between (it split the page suite
  and shortened the screenshots). The per-step numbers below separate the two.

Step walls on this host vary by 10-40 % between runs (the same 65-case sub-shard took 49 s and
81 s in two runs ten minutes apart), so small differences below are noise; CPU seconds are
steadier.

## 1. Measured: what was over ten minutes, and what was close

Nothing outside vibe-talk was over ten minutes of wall except one step; two were just over ten
minutes of CPU. The 86 steps outside vibe-talk summed 8 434 s of wall and 8 156 s of CPU.

| step | wall s | CPU s | note |
|---|---:|---:|---|
| `cross.agentctl.differential` | **628** | 496 | 24 groups, 760 paired checks, run one after another |
| `python-lifecycle.namespace.shard-1` | 508 | 452 | |
| `python.wrkslots.shard-1` | 493 | **637** | 42 % of its CPU is system time |
| `python-lifecycle.namespace.shard-2` | 449 | 394 | |
| `python.wrkslots.shard-0` | 449 | **601** | |
| `rust.agentctl.test` | 443 | 586 | 1 350 cases, one at a time |
| `cross.dagrun.differential` | 399 | 385 | 889 checks, one at a time |
| `python-lifecycle.namespace.shard-4` | 370 | 357 | |
| `python-lifecycle.namespace.shard-3` | 366 | 314 | |
| `cross.herdr-run.differential` | 357 | 259 | |
| `python-lifecycle.namespace.shard-0` | 340 | 309 | |
| `python-lifecycle.host.shard-0` | 338 | 333 | |
| `rust.wrkslots.test` | 279 | 364 | 86 cases, one at a time |
| `python-lifecycle.namespace.shard-5` | 274 | 263 | |
| `rust.dagrun.test` | 234 | 271 | |
| `rust.gh-paced.test` | 227 | 44 | 235 cases, one at a time, mostly waiting |
| `examples.run.rust` | 178 | 103 | |
| `cross.agentctl.herdr-agent-differential` | 169 | 86 | |
| `examples.run.python` | 151 | 99 | |

The twenty slowest cases outside vibe-talk (the slowest case overall is vibe-talk's page suite,
538 s, which the other lane split):

| s | gate | case |
|---:|---|---|
| 86.5 | `rust.wrkslots.test` | `completed_scoped_journal_paths_can_be_reused_by_real_python_retries` |
| 84.6 | `python-lifecycle.host.shard-0` | `test_remove_agent_batch_checks_an_image_backed_slot_with_lsof_before_its_fence` |
| 84.2 | `python-lifecycle.host.shard-0` | `test_remove_checks_an_image_backed_agent_slot_under_the_lock` |
| 80.1 | `examples.run.python` | `examples/08-dagrun-clean-build-sweep.yaml` |
| 75.3 | `examples.run.rust` | `examples/08-dagrun-clean-build-sweep.yaml` |
| 72.0 | `rust.agentctl.test` | `chat_runtime::tests::admission_transaction_recovers_every_maximum_batch_write_boundary` |
| 58.1 | `rust.wrkslots.test` | `real_python_late_refusal_completion_preserves_all_attempt_markers` |
| 52.3 | `rust.agentctl.test` | `chat_runtime::tests::completed_explicitly_closed_requests_churn_beyond_population_cap_without_replay` |
| 52.2 | `rust-packages.rust.agentctl` | `agentctl` (package and smoke) |
| 48.0 | `rust.agentctl.test` | `chat_service::tests::capture_and_recovery_complete_a_retirement_a_scan_left_partway_in_a_full_route_ring` |
| 41.7 | `python-lifecycle.host.shard-2` | `test_remove_agent_batch_removes_disk_image_slots` |
| 38.6 | `python.wrkslots.shard-1` | `test_ownerless_clone.py::test_refuses_live_process_inside_the_clone` |
| 37.7 | `examples.run.rust` | `examples/01-linear-chain.json` |
| 33.1 | `python.wrkslots.shard-0` | `test_proc_status.py::test_lsof_guard_refuses_a_real_live_user_with_attributed_evidence` |
| 29.7 | `rust.wrkslots.test` | `real_python_create_and_import_recoveries_close_their_marker_only_with_a_row` |
| 27.9 | `rust.wrkslots.test` | `real_python_interrupted_finish_is_pending_in_rust` |
| 24.5 | `python-lifecycle.host.shard-0` | `test_remove_agent_batch_refuses_before_fencing_when_the_image_check_outlasts_the_budget` |
| 23.8 | `examples.run.python` | `examples/07-graph-scaling-sweep.yaml` |
| 23.2 | `python-lifecycle.host.shard-0` | `test_remove_refuses_live_process_using_slot` |
| 20.8 | `python.wrkslots.shard-1` | `test_e2e_stress.py::test_real_process_and_git_invariants` |

Per-family totals: pytest 9 591 cases, 4 285 s summed; libtest 5 696 cases, 1 672 s; cross
1 344 checks, 390 s; examples 22, 293 s.

### Where the wall went: the `lifecycle-io` pool

The six lifecycle namespace shards, the three host-visible shards and the two wrkslots component
shards all take one unit of `lifecycle-io`, capped at 5. Their walls summed **3 794 s**, and
3 794 / 5 ≈ 760 s of the 830 s run: that pool, not any one step, set the gate.

The cap dates from 2026-09-25, when the whole lifecycle suite took 190 s and width 6 measured
slower than 5. It was re-measured here by running one 65-case namespace sub-shard (`--shard
5/24`) N times at once:

| copies at once | wall per copy (s) | load average |
|---:|---|---:|
| 1 | 53.2 | 36 |
| 5 | 50.5-51.4 | 46 |
| 8 | 53.9-54.5 | 51 |
| 11 | 56.8-58.1 | 64 |

Eleven at once cost each copy about 7 %, so the contention the cap guarded against is gone.

### Why the wrkslots steps are slow

- **A host-wide `lsof +D` takes 33.6 s here** (11 s user, 22 s system) because about 5 700
  processes are live; it walks every process's descriptors. The host-visible cases that call it
  for real pay that per call: the two 84 s image-backed removal tests make three calls each.
  Inside the namespace shards lsof sees only the shard's own processes and is cheap. These
  cases cannot move into a namespace: they need passwordless `sudo` for loop mounts or
  host-visible PIDs, which is what `ordinary_environment` marks.
- **The component shards are not lsof-bound.** Running `python.wrkslots.shard-0` inside the
  same user and PID namespace as the lifecycle shards was not faster (402 s against 351 s side
  by side) and failed 54 cases that read real host evidence (validation-run liveness, tool
  authority). Their system time comes from process churn across their ~830 cases.
- **The namespace shards are CPU in Python, spread thin.** `perf` over one sub-shard: 62 % of
  samples in the interpreter, 18 % in the kernel on its behalf, 6 % in git. A per-process
  cProfile of every Python process in that sub-shard found no hotspot. Each wrkslots
  invocation costs about 0.2 s to start, about 0.07 s of that creating ~100 dataclasses in
  `cli.py`. The remainder is spread over 1 773 cases that average 1.3 s each.
- **Fixed sleeps are small.** A `time.sleep` recorder in every Python process of all eleven
  steps found 186 s of real sleeping in product code: `_settle_retained_handles`, which waits out
  the 2 s stability window of a just-written validation-run handle, 96 times in the component
  shards. Everything else was the shard supervisors' 10 ms polls (they wait for pytest, so they
  cost nothing) and under 30 s of test fixtures.
- **Moving pytest's temp root from btrfs `/tmp` to tmpfs** did not help measurably (49 s and
  48 s in back-to-back runs of the same sub-shard) and broke one case. Rejected.

### Why the Rust steps are slow

Every Rust suite ran with `RUST_TEST_THREADS=1`, a CI setting the 2026-09-25 overhaul carried
over. `rust.agentctl.test` was 357 s of cases one at a time, plus its build.

### Why `cross.agentctl.differential` is slow

Its 24 groups and 760 paired checks run one after another, each check running the Python edition
and then the Rust edition. Timed per group (on a busier host than the gate's, 794 s in all), the
longest were `_herdr_093_compatibility` 86 s, `_revive_interop` 75 s, `_muse_adoption` 67 s,
`_profiles` 65 s and `_lifecycle` 60 s. Within a group the time is the editions' own waits:
single `start` and `send` calls take 3-12 s against the fixture, polling at their 250 ms
intervals. Neither Python start-up (0.15 s) nor the harness is the cost.

## 2. What changed

| commit | change | evidence |
|---|---|---|
| `validation: let all eleven wrkslots Python steps run at once` | `lifecycle-io` 5 → 11 in both fragments; `host-lifecycle` stays 3 | the table above; in the gate the lifecycle steps' own walls barely moved (namespace shard-1 508 → 471 s) while all eleven ran at once |
| `validation: run three of the longest Rust suites four tests at a time` | `RUST_TEST_THREADS=4` for agentctl, wrkslotsd and gh-paced; one agentctl test fixed | three consecutive four-thread runs per package before the gates, all green |

The agentctl test `recorded_custom_identity_refuses_replacement_process_and_field_mismatches`
failed under parallel tests. Its "duplicate" case named the original process's PID plus one as
a second matching process. That was refused only because nothing usually lived at that PID.
When another test's process did, it was skipped as a mismatch and the assertion failed. The
case now starts a second real process of the same image in the original's process group and
asserts the "multiple matching custom harness processes" refusal itself, so it now checks the
behaviour its name describes.

dagrun's Rust suite stays serial. It passed three four-thread runs by hand. In a gate, inside
the delegated cgroup it runs in, `cli::tests::delegated_parent_clamps_explicit_run_budgets_and_rejects_wide_cpuset`
read a default CPU budget of 1 instead of 2 while other tests ran beside it.

## 3. Before and after

Whole gate: **830.3 s → 612.0 s wall, 11 978 → 8 090 s CPU**, both lanes together. The 86 steps
outside vibe-talk: **8 434 → 6 809 s of wall summed, 8 156 → 6 850 s CPU**.

| step | before wall | before CPU | after wall | after CPU |
|---|---:|---:|---:|---:|
| `cross.agentctl.differential` | 628 | 496 | 571 | 499 |
| `python-lifecycle.namespace.shard-1` | 508 | 452 | 480 | 438 |
| `python.wrkslots.shard-1` | 493 | 637 | 473 | 628 |
| `python-lifecycle.namespace.shard-2` | 449 | 394 | 421 | 377 |
| `python.wrkslots.shard-0` | 449 | 601 | 434 | 596 |
| `rust.agentctl.test` | 443 | 586 | **145** | **216** |
| `cross.dagrun.differential` | 399 | 385 | 348 | 395 |
| `python-lifecycle.namespace.shard-4` | 370 | 357 | 406 | 381 |
| `python-lifecycle.namespace.shard-3` | 366 | 314 | 398 | 336 |
| `cross.herdr-run.differential` | 357 | 259 | 304 | 260 |
| `python-lifecycle.namespace.shard-0` | 340 | 309 | 315 | 294 |
| `python-lifecycle.host.shard-0` | 338 | 333 | 376 | 368 |
| `rust.wrkslots.test` | 279 | 364 | **109** | 331 |
| `python-lifecycle.namespace.shard-5` | 274 | 263 | 307 | 286 |
| `rust.dagrun.test` | 234 | 271 | 206 | 103 |
| `rust.gh-paced.test` | 227 | 44 | **56** | 3 |
| `examples.run.rust` | 178 | 103 | 145 | 106 |
| `cross.agentctl.herdr-agent-differential` | 169 | 86 | 117 | 87 |
| `examples.run.python` | 151 | 99 | 142 | 101 |
| `python-lifecycle.host.shard-2` | 125 | 108 | 132 | 114 |

- **The `lifecycle-io` pool stopped setting the gate.** Its eleven steps summed 3 795 s
  before and 3 830 s after, but they now run at once, so the pool takes as long as its longest
  step: 480 s, against about 760 s before.
- **The Rust steps** dropped by 298 s (agentctl), 170 s (wrkslotsd) and 171 s (gh-paced) of
  wall. agentctl's CPU also fell, from 586 to 216 s. A per-case time in a four-thread run
  includes waiting for the CPU behind its neighbours, so those packages' per-case figures are
  not comparable with the serial ones. The libtest family total fell from 1 672 to 1 438 s
  even so.
- **The new critical path** is `repository.build.rust-launchers` (about 56 s) followed by
  `cross.agentctl.differential` (571 s): about 627 s against the 612 s gate. Behind it are
  the lifecycle shards, at up to 480 s. See item 1 in section 5.

## 4. Pruning

**Nothing was pruned.** No case found here had its signal duplicated elsewhere clearly enough
to remove without a debate. The candidates are in section 5 for the owner.

## 5. For the owner

1. **`cross.agentctl.differential` is now the gate's critical path** (571 s after the 56 s launcher build, in a 612 s gate). Running
   its groups four at a time, with the two editions of each pair at once, gave 24 divergences
   and 30 s timeouts. Cause: every case delivers to the fixture pane `w1:p1`, and both editions
   serialize delivery through a lock file under `~/.local/state/agentctl/target-locks` and
   `/tmp/herdr-agent-target-locks-<uid>`. Both editions resolve those paths from the account
   database, so no environment variable can move them. As a consequence, today:
   - every case of `cross.agentctl.differential` and `cross.agentctl.herdr-agent-differential`
     contends for one host-wide lock with every other checkout's gate on the host;
   - the live account's lock directory collects fixture lock files.

   Options, each a product decision:
   - (a) a lock-root override honoured only by test builds or by an explicit test flag, in
     both editions, so the harness gives each case its own root. Groups could then run four at
     a time.
   - (b) a pane id per case in the fixture and the expected outputs. That is a large harness
     edit.
   - (c) split the step into two DAG steps and accept two-way lock contention, which is what
     `herdr-agent-differential` already has beside it.

   I tried giving each edition private lock directories through user and mount namespaces.
   That was dropped: a fresh namespace per call hides other calls' processes' `/proc/<pid>/exe`,
   and a shared one needs capability handling that the host's guardrails rightly refuse.
2. **Debatable prunes, not made:**
   - `examples/08-dagrun-clean-build-sweep.yaml` compiles dagrun from clean at each width,
     once under each engine: 80 s Python and 75 s Rust, nearly all of it cargo. The engines'
     sweep drivers are what differ. `cross.dagrun.differential` compares them only on cheap
     fixtures (its `sweep*` checks), so dropping one engine's run of example 08 would lose that
     engine's proof against a real build. Saving: about 75 s of a step that is not on the
     critical path.
   - The largest parameterized lifecycle families:
     - `test_agent_removal_refuses_nested_repository_not_provably_disposable`: 31 cases, 97 s.
     - `test_current_incomplete_frozen_requires_pristine_recursive_checkout`: 54 cases, 52 s.
     - `test_validate_nested_git_policy.py::test_audit_and_validation_removal_agree_on_one_tree`:
       7 cases, 67 s.

     Each case is a distinct refusal shape, so trimming them needs whoever owns those refusals
     to say which shapes are equivalent.
   - The two 84 s image-backed removal tests make three host-wide `lsof` scans each by design:
     the test counts them. A cheaper equivalent would count the calls with a stub lsof. That
     would no longer prove the real lsof parses an image-backed slot, which only these tests
     prove.
3. **`_settle_retained_handles`'s 2 s wait** costs about 186 s of the component shards.
   Shortening `_RETAINED_HANDLE_STABLE_NS` for tests that do not examine the window would
   change a product constant under test. The wrkslots owner should decide.
4. **Flakes seen:**
   - `test_validation_run_liveness.py::test_real_host_finish_recovery_waits_for_the_run_process`
     failed once when run beside ten other shards. It reads the real host's processes and
     refused on "mount namespace is indeterminate for live PID N", a process that was not the
     test's own.
   - `cross.agentctl.differential` failed one gate here on a revive dry run blocked by "cannot
     parse process stat" (#495 revive-stat-parse-flake). Neither change touched that code.
5. **Residual risk of the four-thread Rust suites:** a test that writes a fake executable and
   runs it at once can, rarely, see ETXTBSY when another test thread forks between the write and
   the exec. agentctl's `write_executable` and `spawn_retrying` already guard some fixtures.
   `FakeExecutable::set_response`, `install_test_elf` and gh-paced's `Sandbox::new` are not
   guarded. No run here hit it.
