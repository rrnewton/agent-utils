---
title: 'post-provision-hook-slot-types: post-provision hooks cannot be limited to slot types and do not see the slot type'
status: closed
priority: 2
issue_type: feature
created_at: 2026-11-09T11:00:09.000000000+00:00
updated_at: 2026-11-09T13:30:00.000000000+00:00
---

# Description

_run_post_provision_hooks runs every configured hook for every checkout, and the
hook environment (WRKSLOTS_CHECKOUT, WRKSLOTS_CHECKOUT_NAME, ...) carries no slot
type. A consumer wants a dependency-generator hook only in agent slots, never in
validation checkouts. Today its hook has to guess from the WRKSLOTS_CHECKOUT path,
which is fragile.

# Design

- Add an optional per-hook slot_types: list in the config (for example
  [agent]). A hook with no slot_types runs for every slot type, as today.
  Reject an unknown slot type at config load with a clear error.
- Add WRKSLOTS_SLOT_TYPE to the hook environment, for every hook.
- Document both in the user guide, with the config example and the variable.
- Keep older readers working: if the new key needs #1109's ignorable_keys or
  min_wrkslots_version to stay forward compatible, use it and say which.

# Acceptance Criteria

- A hook with slot_types [agent] runs for agent slots and not for validation
  checkouts.
- A hook without slot_types runs everywhere, as before.
- Every hook sees WRKSLOTS_SLOT_TYPE.
- An unknown slot type is refused at config load.
- Python first; Rust parity (mu-rswrk2, devbig014) where Rust runs hooks.

# Notes

Requested by a consumer via the lead, 2026-11-09. Filed by the devbig030
coordinator.

Python delivered (cl-wrkid, 2026-11-09; design agreed with the coordinator):
- A `post_provision_hooks` entry is a command string, as before, or a mapping `{command, slot_types}`. `slot_types` is a non-empty list of `agent`/`validate` without repeats; anything else refuses at config load, naming the entry (`configuration.post_provision_hooks[N]...`). A mapping without `slot_types` runs everywhere. Parser `_post_provision_hook_entries`, selection `_post_provision_hooks_for`, `Config.post_provision_hook_slot_types`.
- Every hook gets `WRKSLOTS_SLOT_TYPE`, in both editions (Rust: create_write.rs).
- The create journal keeps its shape (a list of command strings) and records the hooks effective for its slot type; the three readers that compare journal hooks with the configuration compare against that list.
- Forward compatibility: the key is the existing `post_provision_hooks`, so #1109's `ignorable_keys` cannot help. A reader before 0.8.0 refuses a mapping entry as not a string, which fails closed. This release bumps the version to 0.8.0 in both editions (`__version__`, pyproject, Rust `WRKSLOTS_VERSION`), and the USER_GUIDE's new "Post-provision hooks" section tells users to set `min_wrkslots_version: 0.8.0` with a mapping entry.
- Tests: `test_post_provision_hooks_select_slot_types_and_see_the_type`, `test_an_interrupted_create_journal_records_its_slot_types_hooks`, `test_post_provision_hook_slot_types_refuse_at_config_load` (test_lifecycle.py); `test_create_hooks_see_the_slot_type_in_both_editions`, `test_rust_refuses_a_hook_slot_types_entry_until_it_is_ported` (test_create_write_rust.py).

Remaining, the reason this stays open: Rust parity for `slot_types` (mu-rswrk2, after this lands): config_validate.rs must accept the mapping entry, create_write.rs must run only the slot type's hooks and record that list, and classify_journals.rs must compare journal hooks against it. Until then the Rust edition refuses a configuration with a mapping entry ("post_provision_hooks... must be a string"), never running a hook in the wrong slot type. The test above pins that; flip it into a differential when porting.

## Resolution

[watermelon-full-preview] Rust parity landed with this change. `config_validate.rs` now parses string and mapping hook entries with the Python oracle's exact acceptance and refusal wording, preserves the parallel selection as `post_provision_hook_slot_types` in the normalized config document, and shares one `post_provision_hooks_for(config, slot_type)` selection helper. Create records and runs only the effective hooks, writes a journal progress step after each successful hook exactly like Python, and the completed-create and classify readers compare journal hooks against the journal's own slot type. The pinned refusal differential flips to full Python/Rust parity (agent and validate selection, `WRKSLOTS_SLOT_TYPE`, config conversion in both directions, and an interrupted validate journal classified against its own type).

