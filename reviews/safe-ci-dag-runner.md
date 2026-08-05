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

## Cross-implementation evidence

- `python3 cross/differential.py --tool cpuset-alloc`
  - **31/31 checks passed**.
  - Includes Python-to-Rust and Rust-to-Python live ownership of one shared ledger, disjoint core
    assignments under overlap, identical shared schema, exact release to an empty ledger, mutation
    self-test verdicts, signal status, invalid inputs, wrapped help, and clean missing-executable
    failure.
- `python3 cross/differential.py --tool safe-ci-dag-runner`
  - Default seed/count completed with **403 checks passed across 41 fixtures**.
- `python3 -m mypy cross/differential.py`
  - No issues.

The differential launches compared CLIs in independent sessions/process groups. This prevents an
adversarial CLI or scope manager from taking down the comparison harness and makes signal behavior
an observed result rather than a harness-level side effect.

## Focused test evidence

- Python: **85 passed** across reservation, allocator, CLI, cgroup ownership/exact-set, and build
  job-cap tests; the final allocator/reservation/cgroup subset passed **33 tests** after executable
  preflight hardening.
- Rust: the full safe package suite passed **85 unit tests plus 8 integration tests**, including
  live cgroup memory, CPU-time, core-box, and default-containment tests.
- `python3 scripts/embed_userguides.py --check`: all 12 rendered paired documents and two
  single-language package documents are current and standalone through exact checked links.
- Isolated wheel/crate artifact checks passed for all packages, including entry points, embedded
  resources, licenses, offline startup, and foreign-documentation checks.

## Capability boundary

Hard CPU pinning intentionally requires a working user systemd scope with `AllowedCPUs`. On hosts
without that capability, both packages reserve nothing permanently, launch no workload, explain
the missing hard boundary, and return status 3. This is a refusal, not silent degradation.
