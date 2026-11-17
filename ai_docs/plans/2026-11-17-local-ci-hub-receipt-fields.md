# Receipt-field contract for local-ci-hub (final)

Status: agreed base with mark-ups incorporated. No receipt code
changes until the coordinator relays the consumer's check of this
table. It names the fields one terminal record must carry for the
receipt predicate to qualify a run, who computes each field, and
its type. Project meaning (which profile qualifies, which payload
is current) stays project configuration.

| Field | Type | Computed by | Meaning |
|---|---|---|---|
| `commit` | string (exact commit) | runner | The commit the run validates; never a prefix or branch. |
| `repo` | string | runner | The repository the commit belongs to. The same SHA must never qualify across repositories. |
| `tree` | string | runner | Tree id of the materialized checkout; a current-main floor compares the target tree with the observed main tree. |
| `commit_anchored` | bool | runner | True only when the runner materialized that exact commit and proved HEAD before launch. An in-place run always writes false. |
| `tree_dirty` | bool | runner | True when the checkout gained tracked changes; computed by the runner's contamination check, never trusted from the validator. |
| `admission` | object | runner, under the admission lock | Admission proof with `kind`, `floor_kind`, `floor_sha` and `observed_main_sha`. A predicate refuses a record without it, and refuses a kind it does not know. |
| `tooling_sha` | string | runner | The exact tool tree that ran the validator. A record from unknown tooling does not qualify. |
| `executed_tests` | int (>= 0) | validator, via project adapter | Tests actually executed. Zero with a success exit is no result, never a pass. |
| `passed_tests` | int (>= 0) | validator, via project adapter | Tests passed; qualification requires it to equal `executed_tests`. |
| `skipped_tests` | int (>= 0) | validator, via project adapter | Tests skipped, kept separate from failures so newly-skipped checks work. |
| `failures` | int (>= 0) | validator, via project adapter | Failed tests; qualification requires zero. |
| `profile` | string | project adapter | Selection profile name; values are project data (for example super, full, quick). |
| `selection_mode` | string | project adapter | Selection mode (for example `full`); partial selections do not qualify. |
| `gates_run` / `gates_expected` | int / int | validator, via project adapter | Gate accounting when the validator declares gates; equality required when both present. |
| `coverage` | object (optional) | validator, via project adapter | Planned nodes plus empty zero-executed and absent lists, when reported. |
| `outcome` | string | runner result handling | One of pass, fail, timeout, no_result; host-imposed breaches are no_result. A validator verdict vocabulary maps onto these generically: its passed verdict is pass, its failed verdict is fail, and its could-not-determine verdict is no_result. |
| `finished_at` | string (timestamp) | runner | Terminal write time; orders competing records for one commit. |

Examples (synthetic): a qualifying record carries
`commit_anchored: true`, `tree_dirty: false`, `executed_tests: 12`,
`passed_tests: 12`, `skipped_tests: 0`, `profile: "full"`,
`selection_mode: "full"`, `failures: 0`, `outcome: "pass"`, plus
its `repo`, `tree`, `admission` and `tooling_sha`. A record with
`commit_anchored: false` never qualifies, however green. A record
with `outcome: "no_result"` proves nothing either way.
