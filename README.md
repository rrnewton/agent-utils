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
├── py/
│   ├── bin/<tool>            # command entrypoints (shebang symlinks; no build needed)
│   ├── <tool_pkg>/           # the Python package (mypy strict, zero `Any`)
│   └── pyproject.toml
├── rs/
│   ├── bin/<tool>            # compiled release binaries (created by setup; standalone)
│   ├── Cargo.toml            # workspace
│   └── <tool>/               # the crate
├── cross/                    # randomized py-vs-rs differential tests
└── .github/workflows/        # per-tool py / rs / cross workflows, path-filtered
```

`py/bin/<tool>` and `rs/bin/<tool>` expose the **same command names** (no `-py`/`-rs` suffix). The
top-level `bin/` symlink points at the Rust binaries when Rust is built, otherwise the Python ones.

## Tools

| Tool | Purpose | Status |
|------|---------|--------|
| `safe-ci-dag-runner` | Run a DAG of CI/build steps under nested cgroup CPU/memory boxing, with memory-aware concurrency and always-on CPU/mem/ambient-load logging. | 🚧 early |

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
