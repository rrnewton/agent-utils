# safe-ci-dag-runner and cpuset-alloc adversarial review

## Scope

Reviewed both package implementations of `safe-ci-dag-runner` and `cpuset-alloc`, with emphasis
on tree-wide CPU containment, the shared reservation ledger, CLI failure behavior, stress-mode
sizing, standalone package documentation, and live cross-implementation interoperability.

This review record entered the repository with implementation commit
`5ef91c55036227b5dc2997ef069784f337b4cc5e`.

## Adversarial findings and resolutions

1. **Process affinity was not tree containment.** A wrapped child could call
   `sched_setaffinity` and move from its reserved CPU to another allowed CPU while both wrappers
   still returned success. Both implementations now require an exact cgroup
   `cpuset.cpus.effective` bound and fail closed when it is unavailable. There is no soft-affinity
   fallback.
2. **Reservation exhaustion could fall back to a colliding picker.** Occupying every allowed core
   in the live ledger still allowed `run --cores` to start. Exhaustion now returns operational
   status 3 without launching the DAG or wrapped command.
3. **The hard-pin probe admitted false positives.** The old probe did not require a real escape
   mutation and accepted partial positive coverage. The replacement directly attempts to move a
   live child to an excluded CPU, requires that mutation to be blocked without changing the
   child's allowed set, proves every assigned CPU individually usable, and proves exact affinity
   restoration. Probe subprocesses have bounded timeouts.
4. **Same-sized cpusets were treated as equivalent.** Verification compared cardinality rather
   than CPU identity. It now parses Linux CPU-list syntax and requires exact set equality.
5. **An ambient user scope could be mutated.** Core boxing now changes only a verified runner-owned
   scope or direct managed cgroup. An unrelated ambient `.scope` is never modified.
6. **One release could delete another live reservation.** Two same-process, same-tag reservations
   shared an incomplete identity. Release now matches `(pid, starttime, tag, cores)`, with
   regression tests in both implementations.
7. **CLI edge cases diverged.** Non-positive core counts, negative/non-finite sampling windows,
   missing separators, selftest-only options, wrapped `--help`, missing executables, and
   signal-terminated commands now have tested, clean behavior. Signals use shell status
   `128 + signal`; missing executables return status 3 without a traceback or panic.
8. **Stress parity had two faults.** The differential compared nondeterministic parallel trace
   order, and the Rust stress footprint used a one-byte floor while the Python package used the
   configured default step floor. The comparison now checks the stable report block, and both
   implementations apply the same conservative footprint floor with saturating summation.
9. **Package-facing documentation exposed implementation provenance.** Installed guides,
   module-level documentation, public docstrings/rustdoc, and quickstarts are now standalone.
   The package checks reject sibling-language, unrelated-project, repository-path, and template
   leakage. The Rust crate archive also includes its license.
10. **Nested summary help advertised the wrong action.** Rust intercepted every
    `summary <action> --help` request with the generic summary page, hiding real plan flags and
    showing unrelated build flags. Both engines now expose action-specific build, merge, plan, and
    stats contracts; the differential requires the right flags and rejects cross-action leakage.
11. **Summary action schemas were displayed but not enforced.** Rust discarded unexpected
    positionals and silently defaulted malformed reservoir caps, planner names, and output formats.
    Both editions now enforce the same positive signed-64 ASCII cap grammar and action arity, reject
    missing or invalid values, preserve `--` and empty-assignment behavior, and describe planning
    from summary JSON plus a DAG. Adversarial differentials exercise every failure and edge case.
12. **Teardown behavior had diverged and could be bypassed.** One implementation granted a bounded
    SIGTERM diagnostic window while the other immediately killed; the latter also invoked an
    unqualified `kill` from caller-controlled `PATH`. Teardown now uses direct `kill(2)`, gives the
    same bounded grace once per cancellation batch, ignores zombie leaders when deciding whether a
    cooperative child exited, and escalates. Cgroup boxing is the robust whole-subtree boundary;
    unboxed process-group, parentage, and ownership-nonce sweeps additionally remove ordinary
    environment-preserving setsid/double-fork escapees, but cannot promise containment against a
    child that scrubs its environment. The differential proves the explicit fixture PID is gone
    and its TERM attribution marker survives. The libraries no longer make the host process a
    permanent child subreaper as a hidden side effect.
13. **A child could silently miss its cgroup.** The shell migration wrote `cgroup.procs` with
    `|| true`, so a step could run outside applied limits while cleanup of an empty cgroup appeared
    successful. Migration now fails closed with a distinct pre-command status. Cgroup names use an
    injective byte encoding, preventing two valid DAG tags from sharing a containment directory;
    an encoded name beyond the filesystem component limit is rejected before the user command.
14. **IRQ-budget placement could certify missing samples.** Counter rollback, CPU hotplug between
    snapshots, an unreadable first snapshot, missing per-core IRQ data, and a zero sampling window
    produced exceptions or false zero rates. Both allocators now require a positive finite window
    when a budget is requested, use only successfully measured cores, and reject missing or rolled
    back counters. The self-test forwards the configured IRQ budget instead of silently dropping it.
15. **Default evidence exposed logs and grew without a paired contract.** The native-only `/tmp`
    journal was default-on, permissively created, and unbounded. Both packages now implement the
    same explicit opt-in journal, private per-step raw logs, conservative test-boundary parser, and
    timeout culprit report. Directories and files must be owned private objects; no-follow,
    nonblocking, single-link validation rejects symlinks, hard links, FIFOs, and overlong names.
16. **Malformed reservation state could silently release or rename ownership.** Rust dropped bad
    core entries, truncated oversized PIDs, and coerced tags/timestamps while Python applied a
    different set of coercions and defaults. Both now require the same complete typed record:
    positive bounded PID/starttime, a nonempty unique bounded integer core list, a string tag, and
    a finite non-negative numeric timestamp. Invalid ledgers fail closed without being rewritten.

## Cross-implementation evidence

- `python3 cross/differential.py --tool cpuset-alloc`
  - **58/58 checks passed**.
  - Includes Python-to-Rust and Rust-to-Python live ownership of one shared ledger, disjoint core
    assignments under overlap, identical shared schema, exact release to an empty ledger, mutation
    self-test verdicts, signal status, invalid inputs, wrapped help, and clean missing-executable
    failure, plus 23 malformed-record variants that both refuse without rewriting shared state.
- `python3 cross/differential.py --tool safe-ci-dag-runner`
  - Default seed/count completed with **441 checks passed across 41 fixtures**.
- `python3 -m mypy cross/differential.py`
  - No issues.

The differential launches compared CLIs in independent sessions/process groups. This prevents an
adversarial CLI or scope manager from taking down the comparison harness and makes signal behavior
an observed result rather than a harness-level side effect.

## Focused test evidence

- Python: the focused reservation, allocator, attribution, IRQ sampling, and build job-cap suites
  passed **83 tests**.
- Rust: the full safe package suite passed **120 unit tests plus 24 integration tests**, including
  termination attribution, live cgroup memory, CPU-time, core-box, run-timeout, and containment
  tests.
- `python3 scripts/embed_userguides.py --check`: all 16 rendered paired documents and four
  standalone document checks are current and standalone through exact checked links.
- Isolated wheel/crate artifact checks passed for all packages, including entry points, embedded
  resources, licenses, offline startup, and foreign-documentation checks.

## Capability boundary

Hard CPU pinning intentionally requires a working user systemd scope with `AllowedCPUs`. On hosts
without that capability, both packages reserve nothing permanently, launch no workload, explain
the missing hard boundary, and return status 3. This is a refusal, not silent degradation.
