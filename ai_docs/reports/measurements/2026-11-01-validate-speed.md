# #725 validate-speed: baseline and changes

## Baseline (measured before edits)

Source: existing dagrun step profiles from two active worktrees and gate logs,
restricted to runs since 2026-10-31 with a 24-step scheduler. Times below are
medians, in seconds. CPU is user + system time; it can exceed elapsed time.
These are historical observations, not a controlled before/after experiment.

| Workload | Runs with over 100 completed steps | Gate wall | Summed CPU |
| --- | ---: | ---: | ---: |
| Page refactor | 30 | 717.7 | 3000.7 |
| Rust and repository work | 41 | 899.4 | 11232.3 |

| Step | Samples | Wall | CPU | Failed rows |
| --- | ---: | ---: | ---: | ---: |
| `vibe-talk.shots.screenshots` | 30 | 280.8 | 218.9 | 1 |
| `vibe-talk.check.column-overscroll-browser` | 30 | 226.1 | 49.5 | 0 |
| `vibe-talk.check.approval-card-small-screens-browser` | 30 | 178.5 | 32.3 | 0 |
| `vibe-talk.check.approval-card-desk-browser` | 30 | 166.4 | 25.3 | 0 |
| `vibe-talk.check.approval-card-phone-place-browser` | 30 | 158.3 | 25.9 | 0 |
| `vibe-talk.build.tests` | 30 | 103.7 | 844.0 | 0 |
| `vibe-talk.build.release` | 30 | 61.7 | 292.8 | 0 |
| `python.wrkslots.shard-0` | 43 | 764.4 | 626.1 | 4 |
| `python.wrkslots.shard-1` | 43 | 825.4 | 632.0 | 4 |
| `python.wrkslots.shard-2` | 43 | 820.3 | 612.4 | 7 |
| `python.wrkslots.shard-3` | 43 | 805.4 | 642.3 | 3 |
| `cross.agentctl.differential` | 41 | 590.2 | 508.6 | 4 |
| `rust.agentctl.test` | 41 | 378.5 | 817.5 | 6 |

The page workload's critical constraint is the four-browser resource cap:
the latest profiled run spends 2887.4 seconds across 87 browser steps, giving
a 721.8-second work/cap lower bound before startup dependencies. Its final
step completes at 724.1 seconds. Rust test compilation consumes CPU but does
not explain this browser occupancy. Recent broad Rust runs finish on wrkslots
shards (1187.1 and 1197.6 seconds) or the agentctl differential (1424.9 seconds).

Failures are counted, not all attributed to contention. Wrkslots shard failures,
keyboard geometry checks, and fold-latency's fixed handler budget need separate
investigation; formatting and type failures are implementation failures.
The page sample also has two unit failures, two page-2 failures, and one
keyboard-browser failure. Solo rerun logs demonstrate why failed work must
remain selected until it passes.

## Slow individual cases

Normalized timing summaries in `refactor-gate-16-30.log` and
`gate168-schema3.log` (basenames only; paths and machine identities omitted):

| Case | Seconds | What to investigate |
| --- | ---: | --- |
| `test_wrkviz_build_rust.py::test_build_without_summaries` | 250.7 | Module-scoped Cargo build and build-lock contention |
| `test_agentctl_chat_context_rust.py::test_rust_context_matches_python_over_command_adapter` | 205.2 | Launcher cache invalidation and build locking |
| `test_audit_state_rust.py::test_audit_deletable_matches_python` | 196.6 | Repeated `cargo run` against a shared target |
| `test_agentctl_chat_context_rust.py::test_rust_context_refuses_without_adapter` | 191.7 | Same launcher/build path |
| `test_archive_state_rust.py::test_parse_archive_refusals_name_the_same_problem` (one parameter) | 187.1 | Repeated Cargo invocation |
| `message_cache_serve::a_thread_listed_whole_since_its_count_is_complete_whatever_the_count_says` | 30.0 | Intentional provider waiting versus fixture control |
| Page Done second-batch failure and Undo | 15.7 | Browser fixture timers |
| Page Undo second-batch failure | 12.7 | Browser fixture timers |
| `page_build::the_page_build_suite_passes` | 12.6 | Build contract test; must remain selected for page changes |

## Controlled selection comparison (mu-validate, 2026-11-01)

Same counting method on both trees (steps whose labels intersect the selected
groups, plus `component-*` steps): old is `origin/main` (`0b7b5047`), new is
this branch. An unclassified path still selects every group on both.

| Workload | Old nodes | New nodes | What changed |
| --- | ---: | ---: | --- |
| Page-only (`vibe-talk/web/src/present/card.js`) | 122 | 116 | Skips fmt, clippy, operator mypy, and three script self-tests; every browser, page-build, and embedded-page Rust check still runs |
| Rust-tool (`rs/agent-teams/src/ledger.rs`) | 40 | 24 | The crate's own Rust tests run via its component label instead of every crate's workspace tests |
| Notes-only (`.minibeads/issues/agent-utils-725.md`) | 18 | 18 | Already hygiene-only; unchanged |

## Verification and after measurements

Pending implementation and controlled runs. Full validation remains the
canonical `validation.dag.yaml`; no CI signal changes are planned.
