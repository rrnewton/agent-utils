# agent-utils

A collection of small, standalone command-line tools for build orchestration
and repository automation. Every established tool has an independently
installable distribution. Paired tools share their core command contracts;
installation-specific extensions are identified explicitly below.

The implementations are intentionally independent. Shared fixtures,
differential tests, isolated package checks, and adversarial reviews catch
schema, CLI, output, error, and state-transition drift.

## Paired tools

| Command | Purpose | Python distribution | Rust crate |
|---|---|---|---|
| `dagrun` | Plan, visualize, and execute resource-aware CI DAGs with Linux cgroup containment and profiling. | `dagrun` | `dagrun` |
| `cpuset-alloc` | Reserve disjoint CPU sets and hard-pin benchmark process trees. | Companion command in `dagrun` | Companion binary in `dagrun` |
| `tick-hub` | Evaluate independently cadenced reminders and freshness checks in one deterministic tick. | `tick-hub` | `tick-hub` |
| `pr-landing-planner` | Produce advisory, conflict- and CI-aware pull-request landing plans. | `pr-landing-planner` | `pr-landing-planner` |
| `herdr-run` | Run an allowlisted command in a Herdr pane, outside whatever constrains the caller, with audited, byte-preserving results. An agent whose sandbox blocks the network is one such caller. | `herdr-run` | `herdr-run` |
| `agentctl` | Start named coding agents, delegate follow-up work, inspect goals, and retain direct terminal access. Interactive control requires Herdr. | `agentctl` (also worker, polling Chat, and MCP extensions) | `agentctl` (interactive core and event-driven plugin Chat service) |

Each distribution is independently installable and documented. Its README and
embedded user guide describe only that edition, so package-index users do not
need this source tree or knowledge of the sibling implementation.

## Python-only tools

| Command | Purpose | Python distribution |
|---|---|---|
| `wrkviz` | Build durable, zoomable local timelines from coordinator and subagent transcripts. | `wrkviz` |
| `parallel-experiment-runner` | Run boxed, resource-bounded concurrent seed sweeps through `dagrun`. | `parallel-experiment-runner` |
| `agentctl` extensions | Headless workers in Herdr or tmux, the legacy polling Chat transport and launcher, and an MCP interface over the same sessions. | Included in `agentctl` |

These tools are independently installable and follow the same package
documentation and artifact checks. They are explicit exceptions to the
two-language implementation and behavioral-differential contract.

## Persistent coding agents

`agentctl` gives a person or coordinator one interface for long-lived workers:

```sh
cargo install --path rs/agentctl
agentctl quickstart
agentctl capabilities
agentctl chat quickstart
```

The Python package includes interactive and headless workers, the polling Chat
transport and launcher, and MCP. The Rust crate supplies interactive control and
a durable event-driven Chat host for installed subscription plugins and an
operator-selected outbound helper. Both provide `agentctl userguide` and
per-command help; `capabilities` reports the installed adapters. Interactive
control and either Chat bridge require Herdr. Headless workers can use Herdr or
tmux for their transcript view.

`agentctl` has no dependency on the `herdr-run` shell executor. Both call Herdr
through their own adapters. The compatibility names `herdr-agent`,
`herdr-subagents`, and `herdr-chat` belong to the Python agent-control package.
Start new integrations with `agentctl`.
See [public related work](common/docs/agentctl/RELATED_WORK.md) for comparisons
with other session-control and chat approaches.

## Repository layout

```text
common/docs/       settled documentation, public related work, and rendered editions
ai_docs/transient/ dated working designs and temporary research
cross/             behavioral differential harnesses and shared fixtures
examples/          runnable DAG examples
vibe-talk/         a deployable service, outside the workspaces (see below)
py/                independently publishable Python distributions
rs/                independently publishable Rust crates
scripts/           documentation, package, and dependency contract checks
skills/            thin agent-facing command discovery files
```

Several tools use a shared documentation renderer that combines:

```text
common/docs/<tool>/README.template.md
common/docs/<tool>/USER_GUIDE.template.md
common/docs/<tool>/fragments/{python,rust}/{README,USER_GUIDE}.md
```

It writes authoritative rendered editions under `common/docs/`; package trees
link to those files. `agentctl` instead keeps its CLI documentation assets in
`py/agentctl/`, with the core guide shared by both implementations. Package builders dereference the links into ordinary files,
so every installed artifact is self-contained. Check mode verifies the exact
rendered content, link topology, and absence of sibling-language, source-tree,
unrelated-project, or development-history references:

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
python3 cross/differential.py --tool dagrun
make cross
make check-packages
```

The differential harness runs matching commands over valid, invalid, boundary,
and randomized inputs. Human-oriented help may use idiomatic wording, while
machine schemas, normalized results, exit behavior, and state transitions are
cross-checked as part of the contract. Independent findings and reproducible
evidence are recorded under [`reviews/`](reviews/README.md).

## Services

`vibe-talk/` is a **service**, not a command-line tool: a Rust web server that bridges a voice agent
to Discord channels, deployed as a container rather than installed from a package index. It is
therefore an explicit exception to the two-language, two-package contract above — it has one
implementation, its own Cargo workspace and lockfile, and its own CI workflow rather than a place in
`make check` / `make test`, so its web-server dependency tree cannot perturb the published tools'
MSRV or lockfile. See [`vibe-talk/README.md`](vibe-talk/README.md).

## Package documentation

- [Python distributions](py/README.md)
- [Rust crates](rs/README.md)
- [Adversarial review evidence](reviews/README.md)

## License

MIT — see [LICENSE](LICENSE).
