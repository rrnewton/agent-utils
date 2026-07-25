# cross/ — Python-vs-Rust differential tests

Every tool in this repo is implemented twice. `cross/` proves the two implementations behave
**identically** on the outside.

`differential.py` builds a set of **representative** and **randomized** DAG fixtures and runs
each one through *both* the Python CLI (`python3 -m safe_ci_dag_runner`) and the Rust binary
(`rs/target/release/safe-ci-dag-runner`, falling back to `rs/bin/…`), then diffs the observable
results:

| Command | Assertion |
|---------|-----------|
| `list`, `ascii`, `dot` | stdout **byte-identical** |
| `json` | **parsed-equal** (byte-identity is additionally reported when it holds — in practice it always does for ASCII fixtures) |
| `run` (default, `-j4`) | same **exit code** (the eager-exit passed/aborted split races on timing, so only the exit code is compared here) |
| `run` (serial, `-j1`) | same exit code **and** same `passed / failed / aborted / skipped` counts (serial dispatch is a single deterministic LPT sequence, so the counts are fully reproducible) |
| `--max-mem` sizing | same chosen `-j` and same modeled worst-case footprint |
| `--version` / `--help` / no-args | same exit code (and `--version` stdout byte-identical) |

Randomized fixtures are seeded (`--seed`, default 1234; `--random N` controls how many), so a
failure is reproducible. The randomized DAGs are acyclic (deps only reference earlier steps),
use fast `true` / `false` / `echo` / short-`sleep` commands, and only demand scarce resources
that exist in the caps (an unmet demand would hang the run in both builds).

Note on `--keep-going`: it only suppresses the eager-*abort* of already-in-flight steps; on
any failure BOTH builds set an internal stop flag and launch no new steps, so the counts still
race at `-j > 1`. That is why deterministic count comparison uses `-j1`.

Exit status is nonzero on any divergence.

Usage:

```sh
python cross/differential.py --tool safe-ci-dag-runner
python cross/differential.py --random 40 --seed 99   # more fixtures, different seed
```

The Linux cgroup boxing (`--cgroups`), perf logging (`--perf-dir`), and ambient-load
bucketing remain **Python-only** for 0.1; the Rust `run` uses no per-step boxing (matching
Python's default), so those paths are out of scope for the differential.
