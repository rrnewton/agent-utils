---
title: 'dagrun-model-crate: extract the DAG model and JSON IO into a small crate'
status: open
priority: 1
issue_type: feature
assignee: mu-dagmodel
created_at: 2026-11-13T00:00:00+00:00
updated_at: 2026-11-13T00:00:00+00:00
---

# Description

Extract the pure DAG model, its strict JSON reader and byte-exact JSON writer, a few pure helpers, and the shared env names from `dagrun` into a new small crate `dagrun-model` (library `dagrun_model`). `dagrun` depends on it and re-exports everything under its existing paths, so no current caller changes. A downstream build that vendors these crates byte for byte can then link only the model, its JSON IO, the pure helpers, and `testrun-core`, without pulling in the scheduler, cgroup, CLI runtime, or YAML.

[watermelon-full-preview]

# Acceptance Criteria

- New crate `rs/dagrun-model` (lib `dagrun_model`) depends only on serde, serde_json with `float_roundtrip`, and testrun-core (plus libc only if a moved helper needs it). No build script, no proc macros, no features. A closure check modelled on the testrun-core dependency check guards it and runs in validation; the publishable-crate registry knows the new crate.
- The DAG model types and the strict JSON reader (`dag_from_value`, `dag_from_json`) and byte-exact writer (`dag_to_json`) live in `dagrun_model`; the wire format stays byte-identical, proven by the existing round-trip and corpus tests. The YAML reader and writer stay in `dagrun` as thin wrappers that go through a JSON value and call `dagrun_model`.
- Every symbol in the acceptance list below resolves from `dagrun_model` or `testrun_core` under a stable path, and none of them pulls in the scheduler, cgroup, CLI runtime, or YAML. `dagrun` re-exports each moved item under its old path.
- `testrun-core` re-exports the test-results types at the crate root.
- No behaviour change. The Python edition is untouched and the edition and parity checks stay green.

Acceptance list, restated: 65 symbols grouped by kind. Env names (9): the delegated-cgroup, delegated-unboxed, and direct-cgroup names, the in-scope and scope-unit names, the force-scope-attempt name, the artifact-ready-dir name, the step-started monotonic name, and the outer-run name — all to resolve from `dagrun_model::env`. Model types and methods (about 30): the artifact dep, command-type enum and its unknown variant, the DAG config (including its default and with-steps constructor), the manifest, the published artifact, the resource hint, the result manifest with its two variants, the step type with its result-manifest, structured-results, and tag methods, the step-class enum with its three variants and value method, and the structured test-results manifest with its current, diagnostic, and skipped constructors. JSON IO (2): the strict JSON reader and the byte-exact writer. Pure helpers (7): the result-manifest owner lookup, the inner-jobs command and env rewrites, the resolved wall timeout, the label and tag step selectors, the run-timeout violator check, the monotonic clock read, and the ASCII group rendering. Already in testrun-core (14): the test-counts path env name, the process-group CPU type with its constructor and seconds method, and the test-results variants, constructors, and predicates marked present. Partial in testrun-core (5): the diagnostic schema constant and the attempt-outcome, attempt-result, result, and results types, which today need the `test_results` module path and gain a crate-root path. The Cargo dependency row itself resolves by depending on `dagrun-model` instead of `dagrun`.

# Design

- Land in gated steps: (a) the crate plus the model and JSON IO move with re-exports; (b) the helpers, the env module, and the testrun-core root re-exports; (c) only if still needed, the closure check wiring and an acceptance compile test that imports every listed path from the two small crates alone.
- Moved helpers keep their exact signatures and semantics; selection helpers that today use a transitive-dependency walk keep that walk available inside the small crate rather than reaching back into `dagrun`.
- The YAML functions remain in `dagrun`, parsing into a JSON value and delegating to `dagrun_model`, so the small crate never depends on a YAML parser.

# Notes

Number reserved by coordb. No wall-clock assertions in new tests.
