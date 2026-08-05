# agent-utils

A collection of small, standalone command-line tools for build orchestration
and repository automation. Every established tool has an independently
installable Python distribution and Rust crate with the same command name and
observable behavior.

The implementations are intentionally independent. Shared fixtures,
differential tests, isolated package checks, and adversarial reviews catch
schema, CLI, output, error, and state-transition drift.

## Paired tools

| Command | Purpose | Python distribution | Rust crate |
|---|---|---|---|
| `safe-ci-dag-runner` | Plan, visualize, and execute resource-aware CI DAGs with Linux cgroup containment and profiling. | `safe-ci-dag-runner` | `safe-ci-dag-runner` |
| `cpuset-alloc` | Reserve disjoint CPU sets and hard-pin benchmark process trees. | Companion command in `safe-ci-dag-runner` | Companion binary in `safe-ci-dag-runner` |
| `tick-hub` | Evaluate independently cadenced reminders and freshness checks in one deterministic tick. | `tick-hub` | `tick-hub` |
| `pr-landing-planner` | Produce advisory, conflict- and CI-aware pull-request landing plans. | `pr-landing-planner` | `pr-landing-planner` |

Each distribution is independently installable and documented. Its README and
embedded user guide describe only that edition, so package-index users do not
need this source tree or knowledge of the sibling implementation.

`agent-team-timeline` is a concurrently developed Python prototype for
reconstructing coordinator/subagent histories as a local interactive timeline.
It is intentionally outside the paired packaging and parity contract until its
design settles; this work does not attempt a premature port.

## Repository layout

```text
common/docs/       documentation templates, edition fragments, and prototype guides
cross/             behavioral differential harnesses and shared fixtures
examples/          runnable DAG examples
py/                Python distributions and source tools
rs/                independently publishable Rust crates
scripts/           documentation, package, and dependency contract checks
skills/            thin agent-facing command discovery files
```

For each paired tool, the shared documentation renderer combines:

```text
common/docs/<tool>/README.template.md
common/docs/<tool>/USER_GUIDE.template.md
common/docs/<tool>/fragments/{python,rust}/{README,USER_GUIDE}.md
```

It writes the package README and embedded user guide. Check mode verifies exact
generated content and rejects package pages that leak sibling-language,
source-tree, or unrelated-project references:

```sh
python3 scripts/embed_userguides.py
python3 scripts/embed_userguides.py --check
```

## Development

Build both editions:

```sh
make both
```

Run the repository contract:

```sh
python3 scripts/embed_userguides.py --check
cargo fmt --all --manifest-path rs/Cargo.toml -- --check
make both
make check
make test
python3 -m mypy cross/differential.py
python3 cross/differential.py --tool safe-ci-dag-runner
```

The differential harness runs matching commands over valid, invalid, boundary,
and randomized inputs. Human-oriented help may use idiomatic wording, while
machine schemas, normalized results, exit behavior, and state transitions are
cross-checked as part of the contract. Independent findings and reproducible
evidence are recorded under [`reviews/`](reviews/README.md).

## Package documentation

- [Python distributions](py/README.md)
- [Rust crates](rs/README.md)
- [Adversarial review evidence](reviews/README.md)

## License

MIT — see [LICENSE](LICENSE).
