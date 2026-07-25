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

## Cgroup boxing and the differential

Linux cgroup-v2 boxing is **ON by default** in the Python build (it is the tool's primary
purpose): a bare `run` re-execs inside a transient `systemd-run --user --scope` and caps each
step in its own child cgroup. When cgroup-v2 + a working systemd `--user` scope are unavailable,
the default `run` **errors** (exit 3); `--allow-cgroup-failure` downgrades to a best-effort
UNBOXED run with a visible warning.

Cgroup behavior is environment-dependent (a CI container may have no delegated cgroup or systemd
`--user` scope), so it **cannot** be asserted byte-identically here. Every `run` comparison in
`differential.py` therefore passes **`--allow-cgroup-failure`**, which makes both builds execute
the same deterministic, environment-independent UNBOXED scheduling core — exactly the observable
behavior this differential is meant to pin. Boxing itself is proven separately: by the Python
test suite (`pytest`) and by the Rust boxing smoke test (a step allocating past its cap is
OOM-killed).

The perf logging (`--perf-dir`) and ambient-load bucketing are likewise out of scope for the
byte-identical differential.
