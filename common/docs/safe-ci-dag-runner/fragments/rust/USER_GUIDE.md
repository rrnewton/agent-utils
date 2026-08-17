## Installation and library use

```sh
cargo install safe-ci-dag-runner
```

Rust 1.85 or newer is required. The crate installs both binaries. For library
use, declare the dependency and import the crate as `safe_ci_dag_runner`:

```toml
[dependencies]
safe-ci-dag-runner = "0.13"
```

```rust
use safe_ci_dag_runner::{dag_from_yaml, to_ascii};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dag = dag_from_yaml("steps: []")?;
    println!("{}", to_ascii(&dag, None));
    Ok(())
}
```

Public modules cover containment, CPU reservations, resource sizing, profile
summaries, deterministic planning, scheduling, serialization, and
visualization. The `run_dag` function is the explicit uncontained scheduler;
`run_dag_boxed` applies a caller-supplied cgroup manager when present. The
console command establishes and verifies the outer systemd scope. Enforced
containment requires Linux with cgroup v2 and a delegated systemd user scope.
`run_dag(..., combined_limit)` and `run_dag_boxed(..., combined_limit)` treat
that argument as a compatibility combined active-step and total CPU-core limit.
Use `run_dag_limited(..., max_steps, max_cpus, ...)` or a boxed limited variant
to choose the two values independently. `cap_config_max_cpus` applies the same
total-core cap without starting a run.
