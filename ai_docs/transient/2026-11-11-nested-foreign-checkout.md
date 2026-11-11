# nested-foreign-checkout: design (#1400)

Status: design + fixtures first (2026-11-11). Implementation after
#1302 wrkslots-ignored-evidence-salvage lands, since both sit on the
remove path. Fixtures in `py/wrkslots/tests/test_remove_write_rust.py`.

## Problem

A slot holds a linked worktree of a second repository nested inside
the slot directory (`worktrees/slots/<slot>/<repo>-s5b`), undeclared in
the slot record. The finding is produced in two places:

- `_registry_storage_inconsistencies` kind `git-registration-without-row`
  (a Git-registered path inside the managed root with no row), raised
  per-target by `_assert_target_record_storage_consistent`;
- `_assert_no_cross_repository_target_registrations` (cli.py): for every
  repository any record declares, a registered worktree path inside the
  target slot that no declared checkout of that repository explains
  refuses with "source repository <repo> registers unexpected path
  <path> inside target slot <slot>".

`write-handoff` hits the same gate (`_assert_target_record_storage_consistent`
in `_cmd_write_handoff`) and refuses with exit 3, although writing a
handoff is harmless. `remove`/`retire` refuse through the same helpers.

## Ruling, as three behaviours

1. **write-handoff tolerates and records.** `_assert_target_record_storage_consistent`
   gains a mode that returns tolerated `git-registration-without-row` /
   cross-repository findings for the target slot instead of raising
   (the precedent is `tolerate_target_git_registrations` for
   transfer-owner, and `RegistryStorageFindings.target_git_registrations`).
   write-handoff prints a loud WARNING naming the repository and the
   registered path, and records it in the handoff sidecar:
   `_HandoffArtifact` gains `foreign_checkouts: tuple[...]` of
   `{repository, path}` objects, emitted as `foreign_checkouts` in the
   schema-2 sidecar only when non-empty; `_handoff_artifact_from_obj`
   accepts it as an optional schema-2 key. Removal is unaffected: an
   undeclared nested checkout still refuses there.
2. **remove/retire refusal names the remedy.** The remedy text for this
   finding becomes: declare it with
   `wrkslots adopt-checkout SLOT --repo <name>=<path>
   --coordinator-pid PID --expected-generation N`, or move it out of
   the slot with `git worktree move`. This applies to the
   `_assert_no_cross_repository_target_registrations` remedy and the
   `git-registration-without-row` finding remedy that names this
   condition.
3. **adopt-checkout declares it deterministically.** New subcommand
   `adopt-checkout SLOT --repo NAME=PATH --coordinator-pid PID
   --expected-generation N` (fitting `remove`'s coordinator/generation
   guard conventions). Under the mutation locks it verifies, in order:
   - the slot is an active agent slot at the expected generation, and
     NAME is a valid, unused checkout name;
   - PATH lies inside the slot directory and is a linked worktree
     (`git rev-parse --git-dir` differs from `--git-common-dir`, the
     path is in that repository's `git worktree list --porcelain`);
   - the repository is *known*: it registers PATH, has the configured
     default remote with a URL, and its HEAD is a commit on a branch
     (everything `create` records for a checkout can be recorded);
     anything else refuses as an unknown repository;
   - the worktree is clean by salvage semantics (no tracked or ordinary
     untracked change outside the cache globs; ignored files do not
     dirty it, they are #1302's concern at removal).
   It then appends a `Checkout` (name, project-relative path and
   repository, branch, start_point = HEAD, remote,
   remote_url_sha256, landed_ref, head) via `_replace_record` +
   `_write_active_state(action="checkout-adopted")`. After that,
   `remove` salvages and archives it like any declared checkout,
   including #1302's ignored-file handling, and
   `_assert_no_cross_repository_target_registrations` accepts it
   because `expected_common_by_path` now explains the registration.

## Rust parity

Rust owns `remove` and `write-handoff` writers, so parity is real, not
a note: `handoff_write` tolerates the same finding and writes the same
`foreign_checkouts` sidecar field; `remove_write`'s target-storage
refusal names the same remedy; once adopted, the checkout is an
ordinary record entry and `salvage_gate` handles it (with #1302's
ignored modes) with no new code. `adopt-checkout` itself is a new
record-mutation writer: it lands in Python first, and the Rust edition
refuses it explicitly ("stays with the Python edition") until its own
writer slice, which the differential file records as a parity note in
`rs/wrkslotsd/src/cli.rs`. Fixture tests are differential where both
editions run (remove refusal, remove after adoption) and Python-only
for adopt-checkout's own refusals.

## Fixtures (test_remove_write_rust.py, nested branch)

- `_add_foreign_worktree`: second `_repo` upstream beside the project,
  `git worktree add worktrees/slot-a/foreign -b foreign/work`, pushed.
- write-handoff succeeds, warns naming path+repository, sidecar
  records `foreign_checkouts`; remove refuses naming adopt-checkout
  and `git worktree move`.
- adopt-checkout refuses dirty / non-linked / unknown-repository
  (no default remote) paths; on success the record lists the checkout,
  remove then salvages it, and an ignored evidence file inside it
  follows #1302 (refuses by default, lists under `--discard-ignored`).
