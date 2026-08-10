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
safe-ci-dag-runner = "0.12"
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

For Rust harnesses, cargo-nextest supplies libtest's `--exact TEST` arguments,
so the process snapshot can bind each child to its test. Ordinary `cargo test`
runs several tests inside one shared binary; its process tree alone does not
identify the live test and remains explicitly unattributed.
