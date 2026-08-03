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
│   └── docs/<tool>/          #   USER_GUIDE.md (the ONE editable guide) + README.md
├── examples/                 # small, runnable DAG files (one per core idea), run by either build
├── skills/                   # thin per-tool agent skills (point at `<tool> --userguide`)
├── scripts/                  # build helpers (e.g. embed_userguides.py, run by setup)
├── py/
│   ├── bin/<tool>            # command entrypoints (shebang symlinks; no build needed)
│   ├── <tool_pkg>/           # the Python package (mypy strict, zero `Any`); USER_GUIDE.md embedded here
│   └── pyproject.toml
├── rs/
│   ├── bin/<tool>            # compiled release binaries (created by setup; standalone)
│   ├── Cargo.toml            # workspace
│   └── <tool>/               # the crate (src/embedded_userguide.md is the include_str! target)
├── cross/                    # randomized py-vs-rs differential tests
└── .github/workflows/        # per-tool py / rs / cross / examples workflows, path-filtered
```

`py/bin/<tool>` and `rs/bin/<tool>` expose the **same command names** (no `-py`/`-rs` suffix). The
top-level `bin/` symlink points at the Rust binaries when Rust is built, otherwise the Python ones.

## Tools

| Tool | Purpose | Status |
|------|---------|--------|
| `safe-ci-dag-runner` | Run a DAG (a dependency graph) of CI/build steps under nested cgroup CPU/memory boxing, with memory-aware concurrency and always-on CPU/mem/ambient-load logging. | ✅ py↔rs parity proven by the `cross/` differential: `list`/`ascii`/`dot`/`json` output is **byte-identical**, and `run`/`sweep` behavior, `--only` selection, `--max-mem` sizing, YAML *loading* (isomorphic to JSON), and the auto-logging profile-store **schema** (filenames + CSV headers + line endings) are all cross-checked identical. (`quickstart`/`--help`/`yaml` are human-facing text whose exact wording may differ between builds — only their structure and the exit codes are guaranteed.) Loads DAGs from **JSON or YAML** (auto-detected by file extension; YAML additionally allows comments + multi-line "literate" descriptions). Both builds box each step in a cgroup-v2 sandbox **by default** (pass `--allow-cgroup-failure` to run un-boxed where cgroups are unavailable). Per-step tooling: `run --only TAG` runs exactly one step, `run --profile` prints a per-step timing/memory table, and `sweep --step TAG --jobs 1..N` is a parallel-speedup study. Every run/sweep **auto-logs** resource-usage CSVs to a default profile store (`./.safe-ci-dag-runner/profiles/`; override with `--perf-dir` / `$SAFE_CI_DAG_RUNNER_PROFILE_DIR`, disable with `--no-profile`). The runner **feeds that store back at plan time** — a contention-discounted median `est_duration_s` and a high-percentile `rss_estimate_bytes` override the DAG hints once enough samples exist — and `--planner {greedy-lpt,critical-path}` selects the dispatch order; the `plan` command (`--format json` byte-identical across builds) / `run --show-plan` show the estimates + schedule and are cross-checked identical. It also models each step's **parallel-speedup curve** (`wall` vs. inner `-j`) from multi-width samples — with total-CPU-seconds growth as a work-conservation knee signal — and surfaces a recommended `inner_jobs`, achieved `effective_cores`, and the curve in the plan (byte-identical across builds); the boxed per-step CSV now records rich `effective_cores` / `throttled_s` / `quota_utilization_pct` / contention (`external_cores`, `co_tenants`, `ambient_bucket`) / host+step PSI columns, and `inner_jobs` is resolved to a number (never `"ambient"`). A speedup-aware co-scheduling allocator that *acts* on the model is a scoped follow-on. **Closes the profiling feedback loop on EPHEMERAL CI** via `--profile-sync <backend>`: a pluggable UPLOAD+DOWNLOAD of a **constant-sized, mergeable** profile summary (a bounded per-`(step, inner_jobs)` reservoir; merge = union + deterministic content-hash subsample, so it is commutative/associative and byte-identical py↔rs), with `local:` / `git:` (atomic retry-on-conflict) / `github-artifacts:` (non-atomic) backends and a documented S3/R2 stub. The `summary` subcommand (`build`/`merge`/`plan`/`stats`) exposes the primitives. |
| `tick-hub` | A single scheduled *tick* (one cron/loop/timer heartbeat) that funnels many recurring responsibilities — each on its own cadence — and emits a stable, machine-readable `HEALTH`/`ACTION`/`NOTE`/`ERROR` report for a coordinator or automation to dispatch. Reminders, their shell **gates**, and freshness **health checks** are all caller config (JSON or YAML); nothing project-specific is baked into the engine. | 🐍 **Python-first** (mypy `--strict`, zero explicit `Any`). Reminders carry a cadence (`0` = every tick), optional `requires_flags` gating on the typed per-host **ops-state**, an optional shell gate (`success`/`failure`/`nonempty`/`always`, with `key=value` capture interpolated into the emitted line), and an ACTION/NOTE emit template. The due-logic takes an explicit `now` (deterministic tests); per-reminder last-fired epochs persist to a tiny `key=last_fired_epoch` file, written only on `--flush`. YAML *loading* is isomorphic to the JSON schema (YAML-1.2 core scalars, Norway-safe) and ready for the same py↔rs differential the runner has. **Follow-ups:** a Rust port + `cross/` differential, and switching a caller (DeepScry's ops poller) to consume this tool. |
| `pr-landing-planner` | A conflict-graph + CI-aware, **advisory** pull-request landing planner. It combines real `git merge-tree` conflicts, exact-head validation evidence, `ci-hygiene` versus `gate-policy`, assigned agents, `mechanism:<slug>` overlaps, freshness, holds, and CI diagnosis into deterministic per-PR actions and parallel-safe groups. Local evidence (`locally-validated` or an exact-head clean record) does not wait for a stale merge gate; gate-policy changes escalate even when validated. It NEVER mutates a PR. Subcommands `plan` / `graph` / `status` / `quickstart`; `--format {human,json,actions}`. | 🐍 **Python-first** (mypy `--strict`, zero explicit `Any`). Pure core + pluggable `VcsHost`; `--landing-context` supplies caller-owned exact-head evidence/policy/assignment without baking a project ledger into the engine. Content-identity guards reject PR or evidence drift. |
| `parallel-experiment-runner` | Run **N concurrent seed-sweep workers, BOXED** under `safe-ci-dag-runner`'s two-level cgroup-v2 scope — a chaos search / fuzz sweep / flaky-repro hunt / parameter scan expressed as one command template with a `{seed}` placeholder over a seed range. Fixes the failure where unbounded parallel experiments saturate the host (~470 concurrent processes) and starve their own measurements. **Four guarantees:** (1) **CPU-second** budgets, never wall-derived — `--cpu-timeout` is load-immune `user+sys` from cgroup `cpu.stat`, omit = honest **UNSET**; (2) **declared + enforced** concurrency resolved from lane ∩ live-host-capacity ∩ measured per-worker footprint via a `1→2→4` calibration ramp (downshifts when the lane shrinks); (3) a **derived** per-worker cost estimate up front (or explicit UNSET — never a plausible constant) plus measured wall/CPU actuals after; (4) a **clean kill naming the breach** (`CPU-TIMEOUT` / `MEMORY-CAP` / `TIMEOUT`) via setsid-proof `cgroup.kill`. Each seed becomes one boxed `Step`; **there is no second runner** — containment, teardown, and per-step measurement are all reused. Subcommands `run` / `plan-round` (dry width+DAG) / `quickstart`; `--format {human,json}`; drive by inline flags or `--spec sweep.json`. Boxing is on by default (refuses with exit 3 rather than run unboxed; `--allow-cgroup-failure` opts out). | 🐍 **Python-first** (mypy `--strict`, zero explicit `Any`). Additive generalization of `safe-ci-dag-runner`; stable profile keys fold in mandatory identity dims (`backend`/`kernel_id`/`image_id`/`vcpu`/`guest_memory`/`accelerator`) so estimates stay apples-to-apples, and an unattributed run is *ephemeral* (calibrates but is not persisted). **Follow-ups:** a Rust port + `cross/` differential once a caller adopts it. |
>>>>>>> ca415e6 (Add parallel-experiment-runner: N boxed seed-sweep workers on safe-ci-dag-runner)

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

## Shared docs (DRY) — the CLI is the single source of truth

Each tool's user guide lives **once** under `common/docs/<tool>/USER_GUIDE.md`. That single source is
*embedded into each distributable unit* so the guide travels with the tool through `pip install` /
`cargo install` / crates.io, where `common/docs/` is **not** shipped:

- **Python:** `scripts/embed_userguides.py` copies the guide to `py/<pkg>/USER_GUIDE.md`, declared as
  `package-data` in `pyproject.toml`. The CLI reads it at runtime via `importlib.resources` (a real
  package resource — not a path outside the package).
- **Rust:** the same script copies the guide to `rs/<crate>/src/embedded_userguide.md`, baked in with
  `include_str!`. Keeping it **under `src/`** is what makes the `include_str!` target survive
  `cargo package` (an include pointing outside the crate would break crates.io packaging).

`./setup` runs `scripts/embed_userguides.py` on every build, so a fresh checkout always has the
embedded guides. The embedded copies are **committed derived artifacts** (so CI, which builds/imports
directly without running `./setup`, has them present); `common/docs/<tool>/USER_GUIDE.md` remains the
one editable source. Run `scripts/embed_userguides.py --check` to verify they are in sync (CI does).

Every tool exposes the guide from its own CLI:

- `<tool> quickstart` — short in-CLI getting-started tour.
- `<tool> --help` — commands and flags.
- `<tool> --userguide` — the full embedded guide (the complete reference). For `safe-ci-dag-runner`,
  the py and rs builds embed the identical source, so `--userguide` output is **byte-identical**
  across builds (cross-checked in `cross/differential.py`, alongside `--version`).

The README shown on crates.io / PyPI stays in sync via a symlink from `py/README.md` and
`rs/<crate>/README.md` to `common/docs/<tool>/README.md` (build tooling follows the symlink at
publish time). Only the runtime **user guide** needs the embed treatment, because it must be readable
from an *installed* tool.

## Skills (agent-facing)

`skills/<tool>/SKILL.md` are thin, symlinkable agent skills — each a one-line description plus a
pointer to `<tool> quickstart` / `--help` / `--userguide`. They deliberately do NOT duplicate the
guide; the CLI is the source of truth. See [`skills/README.md`](skills/README.md) for how to link
them into an agent's `.claude/skills/`.

## License

MIT — see [LICENSE](LICENSE).
