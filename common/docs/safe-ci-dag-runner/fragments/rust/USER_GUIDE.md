## Installation and library use

```sh
cargo install safe-ci-dag-runner
```

Rust 1.85 or newer is required. The crate installs both binaries. For library
use, declare the dependency and import the crate as `safe_ci_dag_runner`:

```toml
[dependencies]
safe-ci-dag-runner = "0.12"
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
`run_dag_boxed` and the console command establish containment. Enforced
containment requires Linux with cgroup v2 and a delegated systemd user scope.
