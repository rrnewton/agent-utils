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
{"schema":2,"executed_tests":2,"filtered_tests":5,"results":[{"id":"suite$passes","result":"pass","attempts":1},{"id":"suite$recovers","result":"pass","attempts":2}]}
```

A missing or malformed file leaves the counts and individual results unknown.
Retained schema-1 count-only files remain readable but have no current write
path and provide no individual-result authority. Printing a line that looks
like a libtest summary cannot create receipt evidence in this mode.

`resource_caps` apply within one runner process by default. To apply the same
capacities across independent runners, pass `run --resource-caps-path FILE`.
`DAGRUN_RESOURCE_CAPS_PATH=FILE` is the secondary route for launchers where a
flag is hard to add; the flag wins when both are present. A run using the file
prints its path before any node starts. With neither input, no shared file is
read and enforcement remains process-local.

`FILE` must already exist, be a regular file owned by the effective user, have
one hard link, and be readable and writable. A missing configured file is an
error; dagrun does not create an empty state and pretend global caps are active.
Create an empty file before starting any runners:

```sh
umask 077
printf '%s\n' '{"next_ticket":0,"requests":[]}' > /absolute/path/resource-caps.json
dagrun run --dag pipeline.yaml \
  --resource-caps-path /absolute/path/resource-caps.json
```

The current JSON schema is one object with these fields:

```json
{
  "next_ticket": 2,
  "requests": [
    {
      "pid": 1234,
      "starttime": 987654,
      "request": "1234-987654-1",
      "tag": "build.app",
      "ticket": 1,
      "state": "waiting",
      "resources": {
        "browser": {"demand": 1, "capacity": 2}
      }
    }
  ]
}
```

`next_ticket`, `pid`, `starttime`, and `ticket` are non-negative JSON integers,
except that `pid` and `starttime` must be positive. Request and tag values are
non-empty strings. State is `waiting` or `held`. `resources` is a non-empty
object whose names are non-empty strings; every demand and capacity is a
positive integer, demand must not exceed capacity, and live requests for one
resource must record the same capacity. Request values and ticket values are
unique, and each ticket is less than `next_ticket`.

The file currently has no schema version, boot identity, run identity, or
timestamp. The process start time protects against PID reuse during one boot,
but the file cannot distinguish a stale record from a later boot if the PID and
start time happen to match. The request string is derived from PID, process
start time, and ticket; it identifies a request, not a run. Those omissions,
the strict ticket ordering, and the choice of machine-wide capacities must be
resolved before this should be treated as a generally trustworthy host policy.

This is a repairable design, not one that needs replacement. A next schema
should carry a schema version, the host boot identity, the dagrun run identity,
and the times at which each request was written and granted. The reader should
refuse versions it does not understand, and records from a different boot
should not consume current capacity. The run identity and times should make it
possible to connect a ledger record to the run that produced it and to explain
how long it waited. The machine policy must also establish the capacities used
by every participating runner. Finally, ticket ordering should be revised so a
request that cannot yet fit does not leave otherwise usable capacity idle. The
current locking, atomic file replacement, strict parsing, and process start-time
checks remain useful parts of that repair.

The environment-only implementation that preceded the flag was invisible in
`--help` and startup output. It also treated an absent configured file as an
empty state. Both behaviors made it impossible to tell whether global caps were
actually in effect. The current interface exposes the opt-in and refuses the
missing file. A waiting step does not start its wall or CPU timer until every
requested capacity is granted, and child commands do not inherit the path
because they are already inside the outer step's grant. Malformed state and
conflicting capacities refuse before a node starts.
