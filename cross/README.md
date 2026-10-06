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

The `herdr-agent` and `agentctl` harnesses run every edition with a guard
directory first on `PATH`. It holds a failing stub for each host tool an edition
could reach by a bare name: `agentcloudctl`, `agentterm`, `agy`, `claude`,
`codex`, `gh`, `herdr`, `muse`, `opencode`, `tmux`, and `wrkslots`. A stub records its call,
and the check `harness/no-host-cli` fails when any call was recorded, so a case
must hand both editions a fixture (`--goal-command-json`, for example) rather
than compare two runs of whatever the developer's machine has installed. The
editions also do not inherit the caller's `AGENTCTL_WRKSLOTS_BIN`, `AGY_BIN`,
`CODEX_BIN`, `HERDR_BIN`, or `MUSE_BIN`, whose absolute paths would bypass the
stubs. Both editions run the system Git (`git check-ignore` for profile
configuration), and Git runs helper programs that its configuration names, such
as `core.fsmonitor`, by absolute path. So an edition inherits no `GIT_*`
variable (`GIT_CONFIG_COUNT`, `GIT_CONFIG_PARAMETERS` and the rest), reads no
system or global Git configuration (`GIT_CONFIG_NOSYSTEM=1`,
`GIT_CONFIG_GLOBAL=/dev/null`), and finds no repository above the case directory
(`GIT_CEILING_DIRECTORIES`).

Without `--herdr-bin`, the Rust edition looks `herdr` up on `PATH` and meets the
stub, but the Python edition runs the `herdr` installed in a fixed location such
as `~/bin`, which a stub cannot shadow. The harness therefore does not start the
editions at all for an invocation that names no Herdr file inside the case
directory (symbolic links resolved, with `..` read both as the kernel and the
Rust client read it and as the Python client does; a `--herdr-bin` after a `--`
terminator does not count), unless its command is one that never contacts Herdr
(`capabilities`, `profiles`, `quickstart`, `skill`, `userguide`, or help and
version output). It records the refusal for `harness/no-host-cli` instead.
Every other command, including a deliberately invalid one, passes
`--herdr-bin <HERDR>`. Any `--herdr-bin` that names a file outside the case
directory refuses the invocation, even after a `--`: after a `--` at the start
of the command line, the Python subcommand parser reads the remaining tokens as
options again. `agentctl chat` is refused even with the fixture, apart from its
help, because the Python edition's Chat bridge ignores `--herdr-bin` and always
runs the installed Herdr. The command is found after every option either edition
accepts before it, spaced or with `=`, including the Rust-only `--agentcloudctl-bin`,
`--agentterm-bin`, `--agentcloud-url` and `--from-session`, so
`--from-session=ID chat publish` is refused too; a test reads both parsers to keep
that list complete. After any other option, a `chat` anywhere later refuses the
invocation, since where the command starts is then unknown.

A stub cannot catch a program named by an explicit path either, so the same
refusal applies, wherever the option appears, to an `--agentcloudctl-bin`,
`--agentterm-bin` or `--claude-bin` (Rust edition only) that names a file
outside the case directory, and to a `--goal-command-json` unless it is `[]` or
exactly the fixture goal transport, `["<HERDR>","goal-rpc"]`: its program must be
the case's `fake-herdr` under both readings of `..` (a link to it counts), and its
only argument `goal-rpc`, the mode that answers requests on standard input and
starts nothing. Naming any file inside the case is not enough, because every case
also holds `fake-muse-runtime`, a copy of the Python interpreter.
Without `--goal-command-json`, an edition runs the goal command stored in the
agent record, so the same rule applies to every `goal_command` and
`native_command` field in the `.json` files of each registry the invocation
can read: each `--registry` or `--state` value, and both defaults (`.agentctl`
and `.herdr-agents`) always, since a token after a `--` that only looks like
the option leaves the default in effect. Each registry is read under both
readings of `..`, and it and everything under it, with symbolic links
followed, must stay inside the case directory; a directory that cannot be
listed is refused, because an edition may still open a record in it by name.
The fixture Herdr applies the rule to the custom harness program it runs for
`pane run`, recording a refusal in the same log, and the editions get an empty
standard input, so a request read from it (by `agentctl mcp`) cannot name a
goal command the harness has not checked.

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
lists every skip, and ends with `PARTIAL` instead of `OK`, counting the skips
of each kind: `... 3 check(s) were skipped and are UNVERIFIED (allowed skips by
kind: eight-cpu 1, multi-cpu 2)`. An unknown kind is refused.

| Lane | Coverage |
|---|---|
| `make cross`, invoked by hand | full: every skip fails. No workflow or scheduled job runs it, so the live-scope checks the validation nodes skip run only here ([agent-utils issue 165](https://github.com/rrnewton/agent-utils/issues/165)) |
| validation nodes `cross.dagrun.differential` and `cross.dagrun.cpuset-differential` | partial for `delegated-live-scope` only; boxing stays required |
| hosted repository CI and the nightly full run | additionally partial for `boxing`, because hosted runners run the graph with `--allow-cgroup-failure`, and for `eight-cpu`, because hosted runners have fewer than eight CPUs |

The CPA memory-cap check needs a core budget of eight inside the
`cross.dagrun.differential` node. The node asks for eight
(`preferred_inner_jobs: 8`), and `scripts/validate.py` plans with
`--planner critical-path`, which never changes a step's width from profile
history; only `--planner cpa` does. The node therefore gets fewer than eight
cores only from an input the caller sets: a host with fewer than eight usable
CPUs, `VALIDATE_MAX_CPUS` below eight, `--cores` below eight or `--planner cpa`
in `VALIDATE_DAGRUN_FLAGS`, or a smaller CPU ceiling inherited from an outer
dagrun. Any of these skips the check as `eight-cpu`, and the node, and with it
the local validate run, fails `INCOMPLETE` until the caller declares
`AGENT_UTILS_CROSS_ALLOW_SKIP=eight-cpu`. The node's width stays resizable
rather than fixed at eight, because dagrun refuses a fixed width above the
run's `--max-cpus`, and four-core hosted runners must still run it.

A boxed check counts only when both engines print `cgroup boxing ACTIVE`, which
they do exactly when they have set up per-step cgroups. When the outer scheduler
has no cgroup subtree to delegate, the boxed checks are listed as `boxing` skips
and not run, because a nested engine would run them uncontained.

`--tool all` ends with one line for the whole run, after each tool's own
verdict: `cross: OK - ...`, `cross: PARTIAL - N skipped across tools (TOOL N,
...; by kind: KIND N, ...)`, or `cross: FAILED - ...` naming each tool that did
not pass.

dagrun reports a node by its exit status, so a partial cross node that exits 0
is shown as `✓ PASS`, and the run's last line counts it among the passes. The
node's own `PARTIAL` line appears only in the brackets after that `PASS`.
`scripts/validate.py` therefore sets `AGENT_UTILS_CROSS_COVERAGE_DIR` for the
graph. Each cross verdict leaves a JSON record there, and when any node skipped
checks the run ends with `validate: PARTIAL - every selected node passed, but N
cross check(s) were skipped and are UNVERIFIED (KIND N, ...)`, listing each node
with its own count per kind, instead of `validate: OK`. Its exit status stays 0.
Every selected cross node must leave a
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
