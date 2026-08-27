## Install

```sh
cargo install dagrun
```

Rust 1.85 or newer is required.

The crate installs the `dagrun` and `cpuset-alloc` binaries. Add it as a
library dependency when the model or planning engine belongs inside an
application:

```toml
[dependencies]
dagrun = "0.15"
```

## Rust API

The crate exports the model, strict serializers, planner, scheduler, and
visualization helpers:

```rust
use dagrun::{dag_from_yaml, to_ascii};

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

For receipt-bearing validation, set `DAGRUN_REQUIRE_STRUCTURED_TEST_COUNTS=1`.
The runner then exports a scheduler-owned `DAGRUN_TEST_COUNTS_PATH` to each
step and does not derive functional counts from human-readable output. A
controlled test framework writes exactly one JSON object to that path:

```json
{"schema":1,"executed_tests":23,"filtered_tests":5}
```

A missing or malformed file leaves both counts unknown. Printing a line that
looks like a libtest summary cannot create receipt evidence in this mode.

`resource_caps` normally apply within one runner process. A launcher that permits
several independent runners to share the same scarce resources can set
`DAGRUN_RESOURCE_CAPS_PATH` to one shared state file. The existing capacities
then apply across those processes as well. A waiting step does not start its
wall or CPU timer until every requested capacity is granted, and child commands
do not inherit the path because they are already inside the outer step's grant.
Malformed state and conflicting capacities refuse before a node starts.
