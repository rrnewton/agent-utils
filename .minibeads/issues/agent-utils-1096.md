---
title: 'wrkslots-slot-identity: a configured git identity applied slot-privately to every checkout on create'
status: closed
priority: 2
issue_type: feature
created_at: 2026-11-08T23:40:00.000000000+00:00
updated_at: 2026-11-09T02:30:00.000000000+00:00
closed_at: 2026-11-09T02:30:00.000000000+00:00
---

# Description

Agents working in wrkslots slots commit under whatever git identity they fall through to:

- Nested slots fall through to `~/.gitconfig`, which is the owner's personal identity or a host-derived one.
- In flat slots, a lane that runs plain `git config user.name/email` writes the SHARED `.git/config`, which changes the identity for the main checkout and every sibling slot.

An audit of one consumer's public repository found that more than half of the last 200 commits carry an agent identity that embeds an internal host name, against a minority by the consumer's required bot identity. The consumer works around it today with a long `post_provision_hooks` shell literal.

# Design

- A new optional wrkslots config field `identity: {name, email}`. The value lives in the consumer's config; agent-utils ships no default identity.
- On create, per checkout:
  - the slot's root checkout gets `extensions.worktreeConfig=true` plus `git config --worktree user.name/email` (slot-private, never the shared `.git/config`);
  - each initialised submodule checkout gets plain `git -C <sub> config user.name/email` (that slot's private module config).
- The steps are journaled like other create steps and validated (non-empty name, plausible email). A missing submodule path is reported, not fatal to create.
- No field means no change.
- Python first. Mirror in Rust, or record the parity gap for the Rust wrkslots owner in the same landing.

# Acceptance Criteria

- With `identity` configured, a commit made in a new slot's root and in its submodules carries that identity, and the shared `.git/config` and sibling slots are unchanged (tests for flat and nested layouts).
- Without the field, create behaves exactly as before.
- Invalid values are refused at config validation, and a missing submodule is reported.
- Documented in the config reference and the user guide; Rust parity done or recorded.

# Notes

Requested by the lead (devbig014), 2026-11-08, from a consumer's identity audit. Pairs with #1097 wrkslots-doctor-identity.

## Resolution

Delivered in both editions by cl-wrkid (devbig030), 2026-11-08.

- Config: optional `identity: {name, email}`, validated at load by both editions with the same refusal messages (`_slot_identity`; Rust `config_validate::identity_settings`), documented in `CONFIG_KEY_DOCS` / `config_docs.rs` (with `name`/`email` sub-comments for the literate YAML) and in USER_GUIDE.md "Git identity in new slots".
- Create applies it after the post-provision hooks and before checkout re-verification and registration, so it runs inside the journaled window; `recover` re-applies it after finishing the hooks (every write is idempotent, so no new journal keys: create-journal classification is unchanged in both editions). Root checkout: `extensions.worktreeConfig=true` in the shared config (the one inert shared write) plus `git config --worktree user.name/email`. Each populated submodule (recursive, from `.gitmodules`, initialised = parent has `submodule.<name>.url`) whose Git directory is private to the slot gets plain `git config user.name/email`; initialised-but-missing or non-private submodules are reported on stderr and skipped.
- Pre-mutation refusal when a plan repository's shared config has `core.bare=true` or `core.worktree`: with the extension on, Git applies those to every linked worktree (verified: linked worktrees of a bare repository then fail with "this operation must be run in a work tree").
- Rust: `rs/wrkslotsd` create (`create_write.rs`, the path the soak dispatcher routes `create` to) mirrors the precheck and the apply step with the same stderr lines. Rust has no create-journal resume (`recover` of an interrupted create stays with the Python edition), so there is nothing further to mirror. Differential tests: `test_create_identity_matches_python`, `test_create_identity_refusal_matches_python`, identity cases in `test_config_validate_rust.py` and the `config convert` JSON-to-YAML case.
- Tests (test_lifecycle.py): flat and nested layouts commit under the identity in the root and in recursive submodules while the shared `.git/config` gains only the extension switch and a sibling slot and the main checkout keep theirs; no field leaves config untouched; missing submodule reported; core.worktree refusal; recovery applies it; invalid values refused at load.

