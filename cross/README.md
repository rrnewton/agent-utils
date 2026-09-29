# Behavioral differential tests

`differential.py` runs the independently implemented Python and Rust commands
against the same representative, adversarial, boundary, and seeded-random
inputs. A nonzero exit means the observable contracts diverged, or that a
check did not run in a run that claims full coverage (see "Skipped checks and
partial runs" below).

Run the complete paired-tool contract:

```sh
python3 cross/differential.py --tool all
```

The harness resolves each Rust command through its tracked `rs/bin` Cargo
launcher before starting comparisons. Cargo validates the real workspace cache
and the launcher refreshes source/binary provenance. While holding the same
checkout cache lock, the harness revalidates that provenance and makes a private,
hash-checked executable copy for the tool's full subprocess corpus. Concurrent
cleaning or rebuilding therefore cannot mix Rust versions within a differential.

Run one tool, or increase the randomized corpus reproducibly:

```sh
python3 cross/differential.py --tool dagrun
python3 cross/differential.py --tool tick-hub --random 100 --seed 8675309
python3 cross/differential.py --tool pr-landing-planner --random 100 --seed 8675309
python3 cross/differential.py --tool cpuset-alloc
python3 cross/differential.py --tool herdr-run
python3 cross/differential.py --tool herdr-agent
```

When `--tool` is omitted, the harness checks `dagrun`.

## What is compared

| Tool | Differential contract |
|---|---|
| `dagrun` | Canonical DAG listing, visualization, JSON, YAML loading, validation failures, plan and summary data, selection, successful parallel-speedup sweeps, argument forwarding, stress reports, resource sizing, run outcomes, profile-store schema, CLI surface, and enforcement-capability manifest. |
| `cpuset-alloc` | CLI surface, version, durable-ledger status and reclaim JSON, malformed and boundary arguments, reservation behavior, mutation self-test verdicts, and hard-pin fail-closed behavior. |
| `tick-hub` | Strict JSON/YAML config loading, canonical emission, cadence state, reminder gates, freshness output, flush transitions, CLI failures, numeric boundaries, malformed documents, and randomized tick configurations. |
| `pr-landing-planner` | Fixture collection, graphs, clusters, status and plan output in every format, exact-head/base validation, approval and gate safety decisions, ordering/conflict groups, malformed evidence, numeric boundaries, and randomized PR graphs. |
| `herdr-run` | CLI bootstrap, strict YAML 1.2 configuration and discovery, allow/deny policy, shell-compatible tokenization and inert rendering, terminal-control rejection, malformed inputs, successful dry runs, and byte-identical audit JSONL. Live Herdr protocol/session behavior uses dense fake-client unit suites because production deliberately ignores caller-controlled executable paths. |
| `herdr-agent` | CLI bootstrap, installed guide, exact pane and stable-session targeting, identity assertions, status/read behavior, literal multiline submission, native readiness transitions, durable FIFO state, pending versus possibly-submitted outcomes, malformed-head quarantine, and fail-closed target contradictions. Each package talks through its production client to an isolated executable Herdr protocol fixture. |

Machine-oriented output is compared byte for byte where it is specified as
canonical. YAML emitters and other intentionally idiomatic text are parsed or
checked structurally instead. Concurrent scheduler traces can complete in a
different order, so the harness compares their deterministic final outcome and
report rather than timing-dependent progress lines.

The harness also asks each implementation for its embedded user guide and
checks that the page is complete and does not mention the sibling language or
package manager. Artifact-level wheel and crate checks live in
`scripts/check_python_packages.py` and `scripts/check_rust_packages.py`.

## Cgroup and CPU-set checks

Kernel and service-manager capabilities differ across developer machines and
CI containers. Scheduling-core comparisons therefore opt out of cgroup boxing
explicitly and exercise the deterministic engine. The language-specific test
suites cover cgroup file mutation and cleanup with controlled fixtures.

Hard CPU-set wrappers do not degrade to process affinity. When a host cannot
create and mutation-verify an inescapable subtree scope, both editions must
refuse to launch the workload with the same operational status. On a capable
host, the differential additionally verifies successful reserve/apply/release
behavior. On a host that refuses, the identical refusal counts as a pass and the
reserve/apply/release check is listed as skipped (`hard-cpuset`).

The `selftest` mutation verdict counts only when the probe ran, that is when
both editions report `HARD` or both report `SOFT_OR_INERT`. When both report
`UNTESTABLE`, the identical refusal counts as a pass and
`selftest:mutation-verdict` is listed as skipped: `boxing` when
`systemd-run --user --scope` is unavailable, `multi-cpu` when no core is left
outside the reservation to escape to. An agreed `ERROR`, an `UNTESTABLE` whose
reason differs or is not one of those two, and a verdict whose exit status does
not match it are failures.

## Skipped checks and partial runs

Some checks cannot run everywhere. A skipped check is its own result: it is
never counted as a pass, and the summary lists every skipped check by name with
a total:

```text
cross[dagrun]: SKIPPED/UNVERIFIED 4 check(s), not counted as passes:
  SKIPPED [boxing; REQUIRED] profile-timeseries: the outer scheduler has no cgroup subtree to delegate
  ...
```

| Kind | Why the check did not run |
|---|---|
| `boxing` | no cgroup-v2 boxing: no working systemd `--user` scope, or an outer scheduler with no cgroup subtree to delegate |
| `delegated-live-scope` | the harness runs inside a parent-owned delegated cgroup, where creating a live systemd scope could escape the outer step |
| `hard-cpuset` | both engines refused a HARD cpuset pin on this host |
| `multi-cpu` | fewer than two usable CPUs: one in the CPU affinity mask, or a cgroup CPU quota below two cores |
| `eight-cpu` | a core budget below eight CPUs (the tighter of the CPU affinity mask and any cgroup CPU quota), so the CPA planner's memory cap cannot be shown to bind |

By default a run claims full coverage, so any skip makes it exit nonzero with an
`INCOMPLETE` line. A lane that is partial by design declares which kinds it
accepts, with `--allow-skip KIND[,KIND]` or
`AGENT_UTILS_CROSS_ALLOW_SKIP=KIND[,KIND]`. It says so on its first line, still
lists every skip, and ends with `PARTIAL` instead of `OK`. An unknown kind is
refused.

| Lane | Coverage |
|---|---|
| `make cross`, invoked by hand | full: every skip fails. No workflow or scheduled job runs it, so the live-scope checks the validation nodes skip run only here ([agent-utils issue 165](https://github.com/rrnewton/agent-utils/issues/165)) |
| validation nodes `cross.dagrun.differential` and `cross.dagrun.cpuset-differential` | partial for `delegated-live-scope` only; boxing stays required |
| hosted repository CI and the nightly full run | additionally partial for `boxing`, because hosted runners run the graph with `--allow-cgroup-failure`, and for `eight-cpu`, because hosted runners have fewer than eight CPUs |

A local run on a host with fewer than eight usable CPUs, or with
`VALIDATE_MAX_CPUS` below eight, skips the CPA memory-cap check and ends
`INCOMPLETE` until it declares `AGENT_UTILS_CROSS_ALLOW_SKIP=eight-cpu`.

A boxed check counts only when both engines print `cgroup boxing ACTIVE`, which
they do exactly when they have set up per-step cgroups. When the outer scheduler
has no cgroup subtree to delegate, the boxed checks are listed as `boxing` skips
and not run, because a nested engine would run them uncontained.

`--tool all` ends with one line for the whole run, after each tool's own
verdict: `cross: OK - ...`, `cross: PARTIAL - N skipped across tools (TOOL N,
...)`, or `cross: FAILED - ...` naming each tool that did not pass.

dagrun reports a node by its exit status, so a partial cross node that exits 0
is shown as `✓ PASS`, and the run's last line counts it among the passes. The
node's own `PARTIAL` line appears only in the brackets after that `PASS`.
`scripts/validate.py` therefore sets `AGENT_UTILS_CROSS_COVERAGE_DIR` for the
graph. Each cross verdict leaves a JSON record there, and when any node skipped
checks the run ends with `validate: PARTIAL - every selected node passed, but N
cross check(s) were skipped and are UNVERIFIED`, listing each node, instead of
`validate: OK`. Its exit status stays 0. Every selected cross node must leave a
record: a record that cannot be read, or a selected node that left none (for
example one that `VALIDATE_DAGRUN_FLAGS` narrowed out of the run), is listed as
could not be confirmed and also ends the run with `PARTIAL`. Under GitHub
Actions, where the job still shows a green tick, a `PARTIAL` run also emits a
`::warning` annotation and appends the verdict to the job summary. A graph run
directly with `common/bin/dagrun run` has no such summary: read the bracketed
line of each cross node.

## Fixtures and reproducibility

`cross/yaml_fixtures/` contains YAML scalar, quoting, block-text, duplicate-key,
and numeric edge cases. The harness also consumes bundled examples where they
form useful paired fixtures. Random cases are generated from `--seed`; a failed
seed and case index can therefore be replayed exactly.
