# agent-utils for Rust

This workspace contains the independently installable Rust editions of the
agent-utils command-line tools. Each crate owns its binaries, library API,
version, license, README, and embedded user guide.

| Crate | Binaries | Purpose |
|---|---|---|
| `safe-ci-dag-runner` | `safe-ci-dag-runner`, `cpuset-alloc` | Run and inspect resource-aware CI DAGs; reserve hard-isolated CPU sets for benchmarks. |
| `tick-hub` | `tick-hub` | Evaluate cadenced reminders and health checks in one deterministic tick. |
| `pr-landing-planner` | `pr-landing-planner` | Produce advisory, conflict-aware pull-request landing plans. |

Install one published command with Cargo:

```sh
cargo install safe-ci-dag-runner
cargo install tick-hub
cargo install pr-landing-planner
```

For workspace development:

```sh
cargo build --release --workspace --manifest-path rs/Cargo.toml
cargo test --workspace --manifest-path rs/Cargo.toml
cargo clippy --workspace --all-targets --manifest-path rs/Cargo.toml -- -D warnings
```

Run `./scripts/check_rust_packages.py` from the repository root to build, inspect,
and smoke the registry archive for each crate independently.
