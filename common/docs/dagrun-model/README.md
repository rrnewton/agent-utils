# dagrun-model

The DAG model, its strict JSON reader and byte-exact JSON writer, a few pure
helpers, and the shared env names. A downstream build can link only these
pieces, together with the test-results and process-CPU helpers, without a
scheduler, cgroup runtime, command-line runtime, or YAML parser.

The runner crate depends on this crate and re-exports everything under its
existing paths, so current callers do not change.
