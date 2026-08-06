# Behavioral differential tests

`differential.py` runs the independently implemented Python and Rust commands
against the same representative, adversarial, boundary, and seeded-random
inputs. A nonzero exit means the observable contracts diverged.

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
python3 cross/differential.py --tool safe-ci-dag-runner
python3 cross/differential.py --tool tick-hub --random 100 --seed 8675309
python3 cross/differential.py --tool pr-landing-planner --random 100 --seed 8675309
python3 cross/differential.py --tool cpuset-alloc
python3 cross/differential.py --tool herdr-run
```

When `--tool` is omitted, the harness checks `safe-ci-dag-runner`.

## What is compared

| Tool | Differential contract |
|---|---|
| `safe-ci-dag-runner` | Canonical DAG listing, visualization, JSON, YAML loading, validation failures, plan and summary data, selection, successful parallel-speedup sweeps, argument forwarding, stress reports, resource sizing, run outcomes, profile-store schema, CLI surface, and enforcement-capability manifest. |
| `cpuset-alloc` | CLI surface, version, durable-ledger status and reclaim JSON, malformed and boundary arguments, reservation behavior, mutation self-test verdicts, and hard-pin fail-closed behavior. |
| `tick-hub` | Strict JSON/YAML config loading, canonical emission, cadence state, reminder gates, freshness output, flush transitions, CLI failures, numeric boundaries, malformed documents, and randomized tick configurations. |
| `pr-landing-planner` | Fixture collection, graphs, clusters, status and plan output in every format, exact-head/base validation, approval and gate safety decisions, ordering/conflict groups, malformed evidence, numeric boundaries, and randomized PR graphs. |
| `herdr-run` | CLI bootstrap, strict YAML 1.2 configuration and discovery, allow/deny policy, shell-compatible tokenization and inert rendering, terminal-control rejection, malformed inputs, successful dry runs, and byte-identical audit JSONL. Live Herdr protocol/session behavior uses dense fake-client unit suites because production deliberately ignores caller-controlled executable paths. |

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
behavior.

## Fixtures and reproducibility

`cross/yaml_fixtures/` contains YAML scalar, quoting, block-text, duplicate-key,
and numeric edge cases. The harness also consumes bundled examples where they
form useful paired fixtures. Random cases are generated from `--seed`; a failed
seed and case index can therefore be replayed exactly.
