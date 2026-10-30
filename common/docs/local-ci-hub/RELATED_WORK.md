# local-ci-hub: related work

| System | What it does | Difference |
| --- | --- | --- |
| GitHub Actions / hosted CI | Runs workflows remotely on events | local-ci-hub counts remote results as evidence in the same ledger; it does not replace the remote runner |
| `dagrun` (this suite) | Runs a local dependency graph under resource limits | local-ci-hub records durable receipts and ledger history across runs; a validate command may itself call dagrun |
| `tick-hub` (this suite) | Schedules cadenced health probes | local-ci-hub health probes are executed once per call; scheduling stays with tick-hub |
| Buildkite / Jenkins agents | Self-hosted remote build agents | local-ci-hub targets one dev box used by several agents, with admission slots and exact-commit receipts |

No system surveyed combines per-machine admission, an append-only
Git-backed evidence ledger, and a single exact-commit receipt
verdict spanning local and remote evidence; that combination is
the reason this tool exists.
