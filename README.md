# agent-utils

Small, standalone developer/agent utilities — each implemented **twice**, once in Python and once in
Rust, with the two implementations kept behaviorally identical by randomized differential tests in
CI.

## Why two implementations?

- The **Python** version is easy to hack on, ships as a script, and is the reference for behavior.
- The **Rust** version compiles to a fast, dependency-free static-ish binary suitable for wide reuse
  (and publishing to crates.io).
- **CI cross-checks them:** for every tool, a randomized harness feeds identical inputs to both and
  asserts identical *observable* behavior. A divergence fails the build.

The tools are independent — no shared runtime library, no cross-tool dependencies. Each can be built,
tested, and published on its own.

## Layout

```
agent-utils/
├── setup                     # ./setup [py|rs|both]   (build/typecheck driver)
├── Makefile                  # `make` == ./setup both
├── bin/            ->  rs/bin (or py/bin)   # convenience symlink, created by setup
├── common/                   # language-neutral shared material (single source of truth)
│   └── docs/<tool>/          #   userguide, symlinked into py/ and rs/ for DRY
├── examples/                 # small, runnable DAG files (one per core idea), run by either build
├── py/
│   ├── bin/<tool>            # command entrypoints (shebang symlinks; no build needed)
│   ├── <tool_pkg>/           # the Python package (mypy strict, zero `Any`)
│   └── pyproject.toml
├── rs/
│   ├── bin/<tool>            # compiled release binaries (created by setup; standalone)
│   ├── Cargo.toml            # workspace
│   └── <tool>/               # the crate
├── cross/                    # randomized py-vs-rs differential tests
└── .github/workflows/        # per-tool py / rs / cross / examples workflows, path-filtered
```

`py/bin/<tool>` and `rs/bin/<tool>` expose the **same command names** (no `-py`/`-rs` suffix). The
top-level `bin/` symlink points at the Rust binaries when Rust is built, otherwise the Python ones.

## Tools

| Tool | Purpose | Status |
|------|---------|--------|
| `safe-ci-dag-runner` | Run a DAG (a dependency graph) of CI/build steps under nested cgroup CPU/memory boxing, with memory-aware concurrency and always-on CPU/mem/ambient-load logging. | ✅ py↔rs parity proven by the `cross/` differential: `list`/`ascii`/`dot`/`json` output is **byte-identical**, and `run`/`sweep` behavior, `--only` selection, `--max-mem` sizing, YAML *loading* (isomorphic to JSON), and the auto-logging profile-store **schema** (filenames + CSV headers + line endings) are all cross-checked identical. (`quickstart`/`--help`/`yaml` are human-facing text whose exact wording may differ between builds — only their structure and the exit codes are guaranteed.) Loads DAGs from **JSON or YAML** (auto-detected by file extension; YAML additionally allows comments + multi-line "literate" descriptions). Both builds box each step in a cgroup-v2 sandbox **by default** (pass `--allow-cgroup-failure` to run un-boxed where cgroups are unavailable). Per-step tooling: `run --only TAG` runs exactly one step, `run --profile` prints a per-step timing/memory table, and `sweep --step TAG --jobs 1..N` is a parallel-speedup study. Every run/sweep **auto-logs** resource-usage CSVs to a default profile store (`./.safe-ci-dag-runner/profiles/`; override with `--perf-dir` / `$SAFE_CI_DAG_RUNNER_PROFILE_DIR`, disable with `--no-profile`). The runner **feeds that store back at plan time** — a contention-discounted median `est_duration_s` and a high-percentile `rss_estimate_bytes` override the DAG hints once enough samples exist — and `--planner {greedy-lpt,critical-path}` selects the dispatch order; the `plan` command (`--format json` byte-identical across builds) / `run --show-plan` show the estimates + schedule and are cross-checked identical. It also models each step's **parallel-speedup curve** (`wall` vs. inner `-j`) from multi-width samples — with total-CPU-seconds growth as a work-conservation knee signal — and surfaces a recommended `inner_jobs`, achieved `effective_cores`, and the curve in the plan (byte-identical across builds); the boxed per-step CSV now records rich `effective_cores` / `throttled_s` / `quota_utilization_pct` / contention (`external_cores`, `co_tenants`, `ambient_bucket`) / host+step PSI columns, and `inner_jobs` is resolved to a number (never `"ambient"`). A speedup-aware co-scheduling allocator that *acts* on the model is a scoped follow-on. |
| `tick-hub` | A single scheduled *tick* (one cron/loop/timer heartbeat) that funnels many recurring responsibilities — each on its own cadence — and emits a stable, machine-readable `HEALTH`/`ACTION`/`NOTE`/`ERROR` report for a coordinator or automation to dispatch. Reminders, their shell **gates**, and freshness **health checks** are all caller config (JSON or YAML); nothing project-specific is baked into the engine. | 🐍 **Python-first** (mypy `--strict`, zero explicit `Any`). Reminders carry a cadence (`0` = every tick), optional `requires_flags` gating on the typed per-host **ops-state**, an optional shell gate (`success`/`failure`/`nonempty`/`always`, with `key=value` capture interpolated into the emitted line), and an ACTION/NOTE emit template. The due-logic takes an explicit `now` (deterministic tests); per-reminder last-fired epochs persist to a tiny `key=last_fired_epoch` file, written only on `--flush`. YAML *loading* is isomorphic to the JSON schema (YAML-1.2 core scalars, Norway-safe) and ready for the same py↔rs differential the runner has. **Follow-ups:** a Rust port + `cross/` differential, and switching a caller (DeepScry's ops poller) to consume this tool. |

## Building

```sh
./setup              # build & check both implementations (default)
./setup py           # Python only: mypy-strict typecheck + wire py/bin
./setup rs           # Rust only:   cargo build --release -> rs/bin
./setup rs --clean   # ... then delete rs/target (binaries remain in rs/bin)
make                 # == ./setup
make check           # mypy (strict) + cargo clippy -D warnings
make test            # pytest + cargo test
```

On a Meta host, prefix any network step with `with-proxy` (see the `with-proxy` skill): external
package fetches (crates.io, PyPI) must egress through fwdproxy.

## Shared docs (DRY)

Each tool's userguide lives once under `common/docs/<tool>/` and is symlinked into `py/<tool>` and
`rs/<tool>` so the crates.io and PyPI READMEs stay in sync. (If publishing tooling refuses to follow
those symlinks, the publish step generates a copy instead — tracked as a known trade-off.)

## License

MIT — see [LICENSE](LICENSE).
