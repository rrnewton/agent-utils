# cross/ — Python-vs-Rust differential tests

Every tool in this repo is implemented twice. `cross/` proves the two implementations behave
**identically** on the outside.

`differential.py` runs a set of invocations (and, as the runner is ported, randomized inputs) through
*both* the Python and the Rust build of a tool and diffs the observable results — exit code, stdout,
and (for the runner) the load-bearing decisions:

1. RAM budget + DAG → chosen `-j` (concurrency)
2. dispatch order for identical duration hints
3. failure-cause precedence (OOM > timeout > pids-guard > detail-failure > signal > exit)
4. ambient-load bucket thresholds (quiet / moderate / busy)
5. CSV column order + self-migrating header
6. cgroup enforcement-kind decision table

These are pure functions over injected inputs (a DAG plus fake `/proc/meminfo`, `/proc/stat`, and a
fake cgroupfs), so the differential runs on any Linux CI runner without needing real delegated
cgroups.

Usage:

```sh
python cross/differential.py --tool safe-ci-dag-runner
```

Exit status is nonzero on any divergence.
