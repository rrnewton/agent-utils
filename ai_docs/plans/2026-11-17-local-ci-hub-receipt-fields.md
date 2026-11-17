# Receipt-field contract for local-ci-hub (draft for agreement)

Status: draft. No receipt code changes until the coordinator agrees
this contract with the consumer. It names the fields one terminal
record must carry for the receipt predicate to qualify a run, who
computes each field, and its type. Project meaning (which profile
qualifies, which payload is current) stays project configuration.

| Field | Type | Computed by | Meaning |
|---|---|---|---|
| `commit` | string (exact commit) | runner | The commit the run validates; never a prefix or branch. |
| `commit_anchored` | bool | runner | True only when the runner materialized that exact commit and proved HEAD before launch. An in-place run always writes false. |
| `tree_dirty` | bool | runner | True when the checkout gained tracked changes; computed by the runner's contamination check, never trusted from the validator. |
| `executed_tests` | int (>= 0) | validator, via project adapter | Tests actually executed. Zero with a success exit is no result, never a pass. |
| `passed_tests` | int (>= 0) | validator, via project adapter | Tests passed; qualification requires it to equal `executed_tests`. |
| `profile` | string | project adapter | Selection profile name (for example `full`); which values qualify is project data. |
| `selection_mode` | string | project adapter | Selection mode (for example `full`); partial selections do not qualify. |
| `failures` | int (>= 0) | validator, via project adapter | Failed tests; qualification requires zero. |
| `gates_run` / `gates_expected` | int / int | validator, via project adapter | Gate accounting when the validator declares gates; equality required when both present. |
| `coverage` | object | validator, via project adapter | Planned nodes plus empty zero-executed and absent lists, when reported. |
| `outcome` | string | runner result handling | One of pass, fail, timeout, no_result; host-imposed breaches are no_result. |
| `finished_at` | string (timestamp) | runner | Terminal write time; orders competing records for one commit. |

Examples (synthetic): a qualifying record carries
`commit_anchored: true`, `tree_dirty: false`, `executed_tests: 12`,
`passed_tests: 12`, `profile: "full"`, `selection_mode: "full"`,
`failures: 0`, `outcome: "pass"`. A record with
`commit_anchored: false` never qualifies, however green. A record
with `outcome: "no_result"` proves nothing either way.
