## Install

```sh
cargo install safe-ci-dag-runner
```

Rust 1.85 or newer is required.

The crate installs the `safe-ci-dag-runner` and `cpuset-alloc` binaries. Add it
as a library dependency when the model or planning engine belongs inside an
application:

```toml
[dependencies]
safe-ci-dag-runner = "0.15"
```

## Rust API

The crate exports the model, strict serializers, planner, scheduler, and
visualization helpers:

```rust
use safe_ci_dag_runner::{dag_from_yaml, to_ascii};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dag = dag_from_yaml("steps: []")?;
    println!("{}", to_ascii(&dag, None));
    Ok(())
}
```

`run_dag(..., combined_limit)` keeps a compatibility combined setting: that
number bounds active steps and caps each runner-controlled step's width. Call
`run_dag_limited(..., max_steps, max_cpus, ...)` (or the corresponding boxed
limited helper) when those limits differ. `cap_config_max_cpus` exposes the same
per-step capping policy for runner-controlled commands. It deliberately
leaves a self-managed fixed width unchanged; the run helpers then reject it if
it exceeds `max_cpus`. Library helpers do not establish an outer scope, so
`max_cpus` is a whole-run bandwidth cap only when the caller supplies equivalent
outer containment; it does not serialize steps whose declared widths sum above
that value.

For Rust harnesses, cargo-nextest supplies libtest's `--exact TEST` arguments,
so the process snapshot can bind each child to its test. Ordinary `cargo test`
runs several tests inside one shared binary; its process tree alone does not
identify the live test and remains explicitly unattributed.
