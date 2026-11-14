---
title: 'local-ci-hub-deployment-config: documented acceptance and deployment setup for the Rust front door'
status: closed
priority: 1
issue_type: task
created_at: 2026-11-14T03:41:38.000000000+00:00
updated_at: 2026-11-14T05:28:06.000000000+00:00
closed_at: 2026-11-14T05:28:06.000000000+00:00
---

# Description

The Rust `local-ci-hub` front door (ledger rows, verdicts and digest-rows)
needs deployment data that today lives only in flags: the ledger repository
and pinned head, the evidence archive root, the field names a deployment
uses for derived evidence bindings, required base evidence and typed
evidence heads, the admission-fixture producers to exclude, and the index
location. Under the built-in defaults a deployment whose vocabulary differs
refuses with `cell 0 has no derivable evidence binding`, folds fixture
events as real rows, judges verdicts against the wrong base-evidence set,
or emits generic sorted digests where the oracle emits typed struct-order
digests. The flags that prevent each outcome were discoverable only from
an out-of-tree acceptance harness. Any deployment should be able to find,
record and replay them from this repository alone.

# Design

- Document every deployment flag (`--repo`, `--git-ref`, `--evidence-root`,
  `--binding-tree-field`, `--base-field`, `--fixture-producer`,
  `--evidence-head-field`, `--index`) in `rs/local-ci-hub/README.md`: what
  each means, its default, and exactly what breaks without the deployment
  value.
- Add a JSON deployment config and a `--config FILE` front-door option.
  Config keys are `repo`, `git_ref`, `evidence_root`,
  `binding_tree_field`, `base_fields`, `fixture_producers`,
  `evidence_head_field` and `index`. Precedence is explicit flag, then
  config, then built-in default. Repeatable flags take the whole list from
  the winning source. Unknown keys, wrong types and unreadable files are
  usage errors (exit 2), never silent ignores.
- Ship `rs/local-ci-hub/deployment.example.json` with placeholder values
  only. It names no deployment, field vocabulary, host or real path, and
  the client-names check must pass over it.
- Ship a generic acceptance script under `rs/local-ci-hub/scripts/`,
  parameterized by the deployment config and by caller-supplied oracle
  commands. It compares folded terminals, verdict agreement (looser must
  be zero, stricter listed by commit), per-run digests, and index
  stability (cold, indexed and warm-indexed verdicts byte-identical). No
  deployment path or oracle path is baked in.
- Keep all deployment vocabulary as data. No production path hard-codes a
  deployment field name, producer name or path.

# Acceptance Criteria

- `rs/local-ci-hub/README.md` documents all eight deployment flags with
  meaning, default and failure mode, including the
  `no derivable evidence binding` refusal.
- `local-ci-hub --config FILE` loads the example shape; explicit flags
  override config values; config values override defaults; malformed
  configs refuse with a usage error.
- The example config parses and maps to the documented flags in an
  automated test that uses only the committed example and temporary
  files. The test asserts no wall-clock or timing property.
- The acceptance script runs oracle-versus-Rust comparisons and the
  index-stability check from config plus oracle commands alone, and
  fails closed (non-zero exit, named mismatch) on any divergence.
- Validation passes for the touched paths and the client-names check
  passes over every new file.

# Notes

M1 acceptance was measured out of tree against a pinned consumer ledger;
only generic code, placeholder examples and synthetic fixtures belong in
this repository. Tests use temporary directories and temporary git
repositories only.

## Resolution

[watermelon-full-preview] Landed the deployment setup. `rs/local-ci-hub/README.md` now documents all eight deployment flags (`--repo`, `--git-ref`, `--evidence-root`, `--binding-tree-field`, `--base-field`, `--fixture-producer`, `--evidence-head-field`, `--index`) with meaning, default and failure mode, including the `no derivable evidence binding` refusal and a prominent warning that a wrong `--evidence-head-field` silently yields generic-fallback digests that look unchanged. A new `src/deployment.rs` owns the JSON deployment config (unknown keys, wrong types and empty field names refuse as usage errors); the front door gained `--config FILE` with explicit-flag, then config, then default precedence. `deployment.example.json` carries placeholder values only and names no deployment, field vocabulary, host or real path. `scripts/acceptance.py` compares folded terminals, verdict agreement (looser must be zero, stricter listed), per-run digests and cold/indexed/warm index stability from the config plus three caller-supplied oracle commands, with nothing deployment-specific baked in. `tests/deployment_config.rs` proves the example parses and maps to the documented flags and that the binary reads a config file, honours flag-over-config precedence and refuses a malformed config (temporary repositories only). The companion head-field warning in `digest-rows` landed separately as #1510 local-ci-hub-nested-canonical-digest. Crate tests, clippy and rustfmt pass; local validate and the full gate pass on the gated tree.
