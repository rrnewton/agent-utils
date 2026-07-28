# pr-landing-planner

A conflict-graph + CI-aware, **advisory** pull-request landing planner. Given the open pull requests
targeting a base branch, it computes — in one shot — which PRs truly conflict, which red CI results
are real versus benign, how stale each green PR is, which PRs are held, and a recommended per-PR
action, ordered by priority. It **never** arms a merge queue, re-fires a gate, or merges anything: it
reports and recommends; a landing skill / coordinator acts.

Part of [agent-utils](https://github.com/rrnewton/agent-utils). Python-first (mypy `--strict`, zero
explicit `Any`); a Rust port + `cross/` differential is a follow-up.

## Why

It fuses what three predecessor scripts did separately — a conflict/ordering graph, real
`git merge-tree` conflict detection, and per-PR CI/label health — into the one thing none of them
computed: **a fused landing plan** that combines the conflict graph with live CI health, freshness,
and priority.

The headline value is classifying **why** a PR's CI is red, into five modes grounded in real
incidents, so a lander does not treat benign gate noise as a failure:

| Class | Action |
|-------|--------|
| `real` | `hold-fix` |
| `flaky` (matches a caller signature) | `refire-ci` |
| `stale-required-check` (CI green, gate frozen) | `refire-stale-gate` |
| `evaluate-once-race` (gate fired while CI queued) | `wait` (benign) |
| `runner-outage` (gate job never ran) | `escalate-runner-outage` |

## Install & use

```sh
pip install "git+https://github.com/rrnewton/agent-utils#subdirectory=py"

pr-landing-planner quickstart                 # getting-started tour (no repo needed)
pr-landing-planner quickstart --emit-demo > demo.yaml
pr-landing-planner plan --fixture demo.yaml   # a full plan, network-free
pr-landing-planner plan --repo OWNER/NAME --base integration \
    --net-wrapper with-proxy --gh-cmd ./scripts/gh_human
pr-landing-planner --userguide                # the complete reference
```

Subcommands: `plan` (default), `graph`, `status`, `quickstart`. Formats: `--format {human,json,actions}`.

The `actions` format is tick-hub-integrable (Option B, zero tick-hub change): a tick reminder runs
`pr-landing-planner plan --format actions` as a capturing gate and dispatches a landing skill. See
the [user guide](USER_GUIDE.md) for the recipe.

## License

MIT.
