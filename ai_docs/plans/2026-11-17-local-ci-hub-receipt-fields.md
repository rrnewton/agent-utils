# Receipt-field contract for local-ci-hub (revised)

Status: revised after the consumer's check. No receipt code
changes until the coordinator relays the consumer's check of this
revision. It names the fields one terminal record must carry for
the receipt predicate to qualify a run, who computes each field,
and its type. One disagreement is recorded openly below rather
than adopted silently.

| Field | Type | Computed by | Meaning |
|---|---|---|---|
| `schema_version` | int | runner | Record schema version; it sets which thresholds below apply (for example the schema-5 requirements). |
| `producer` | string | runner | The registered writer that produced this record; unregistered producers do not qualify. |
| `record_id` | string | runner | Identity of this record. |
| `run_id` | string (schema 6+) | runner | Identity of the run this record terminates. |
| `commit` | string (exact commit) | runner | The commit the run validates; never a prefix or branch. |
| `repo` | string | runner | The repository the commit belongs to. The same SHA must never qualify across repositories. |
| `tree` | string | runner | Target-tree binding of the materialized checkout (the target tree only; the current-main floor compares commits, below). |
| `base_sha` / `base_tree` | string / string | runner | Base commit and tree the run was taken against. A secondary repository carries its own additional base sha/tree pair under the same rule. |
| `commit_anchored` | bool | runner | Must be true to qualify: the runner materialized that exact commit and proved HEAD before launch. An in-place run always writes false and never qualifies. |
| `tree_dirty` | bool | runner | Must be false to qualify; computed by the runner's contamination check, never trusted from the validator. |
| `admission` | string | runner | The admission kind by name. A predicate refuses a record without it, and refuses a kind it does not know. |
| `admission_floor_evidence` | object | runner, under the admission lock | Floor proof: the floor tree, the floor kind (one of current-main, frozen-target, main-ancestor, disjoint-main, queue-base), the authority binding, plan digests and pin bindings. For current-main the floor is commit equality: the floor SHA equals the observed main SHA. |
| `concurrent_validates` | int | runner | Validations running when this run was admitted. |
| `concurrency_proof` | object | runner | Evidence backing the concurrency count. |
| `tooling_sha` | string | runner | The exact tool tree that ran the validator. A record from unknown tooling does not qualify. |
| `cwd`, `host`, `started_at`, `log_identity` | string | runner | Working directory, host identity, start time, and the identity that binds the run's log. |
| `executed_tests` / `passed_tests` | int (>= 0) | validator, via project adapter, read as test totals | Tests executed and passed. Zero executed is refused, never a pass. The pass rule keeps the owner-approved known-gap exception: a shortfall qualifies only when every missing test is ledgered as a known gap. |
| `filtered_tests` | int (>= 0) | validator, via project adapter | Tests filtered out, kept separate from failures so newly-filtered checks work. |
| `failures` | int (>= 0) | validator, via project adapter | Failed tests; qualification requires zero. |
| `profile` / `selection_mode` | string | project adapter | Qualification requires both to be `full` today; any other profile value, however broad, does not qualify. |
| `gates_run` / `gates_expected` | int / int | validator, via project adapter (`gates[]`) | Required at schema 5+. The accounting must close: run plus intentionally skipped equals expected, with skipped, dependency-skipped and unaccounted node lists named; an over-run does not qualify. |
| `coverage` | object | validator, via project adapter | Required at schema 5+: planned test nodes greater than zero, with zero-executed and absent node lists empty. |
| `cell_results` | object | validator, via project adapter | Per-cell results, bound to this `run_id` and `commit`. |
| `release_builder` / `e2e_payload` | string / object | validator, via project adapter | Release identity: the builder name and the payload it built; only the configured current payload qualifies. |
| `result` | string | runner result handling | One of pass, fail, timeout, incomplete, killed, no_result. A validator verdict vocabulary maps onto these generically: its passed verdict is pass, its failed verdict is fail, and its could-not-determine verdict is no_result. Host-imposed breaches are no_result. |
| `finished_at` | string | runner | Fixed-width UTC RFC3339 ending in `Z`; readers shard on its first 7 characters. |

Recorded disagreement (writer side, kept deliberately): writers
emit only the canonical `result` spelling `fail`. One reader is
known to also accept `failed`; that leniency stays a reader
choice and must not become a second writer spelling, or two
spellings for one verdict will drift.

Examples (synthetic): a qualifying record carries
`commit_anchored: true`, `tree_dirty: false`, `executed_tests: 12`,
`passed_tests: 12`, `filtered_tests: 0`, `profile: "full"`,
`selection_mode: "full"`, `failures: 0`, `result: "pass"`, plus
its `repo`, `tree`, `admission`, `admission_floor_evidence` and
`tooling_sha`. A record with `outcome`-style no result
(`result: "no_result"`) proves nothing either way.
