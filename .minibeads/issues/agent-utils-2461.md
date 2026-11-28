---
title: 'wrkslots-rust-storage-name-parity: Rust create reserves only validate, and Rust audit has no storage-name filter'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- rust
- parity
created_at: 2026-11-28T07:50:00-05:00
updated_at: 2026-11-28T07:50:00-05:00
---

# Description

Follow-up from the #2449 review. The control-directory entries that hold storage rather than being slots are `slot-images`, `slot-state`, `box-state`, `wrkslots-salvage` and `proof-mirrors`. Python lists them in `_REPRESENTATION_DIRECTORIES`; Rust lists them in the status census (`REPRESENTATION_DIRECTORIES` in `rs/wrkslotsd/src/storage_state.rs`). Two other places in the Rust edition do not apply the list:

- **Rust create** (`rs/wrkslotsd/src/create_write.rs`) reserves only `validate`. Python's `_assert_agent_and_slot_free` refuses all five storage names in the nested layout, when the slot directory's parent is the control directory. Rust create runs only under the engine opt-in today.
- **Rust audit's slot enumeration** (`rs/wrkslotsd/src/audit_state.rs`, both enumerations of the managed roots) skips only `EVENTS.*` and the validate root. It has no storage-name filter at all, so in the nested layout it would list storage directories as unregistered slots. Python audit skips them through `_is_representation_directory`. Audit is not Rust-routed by default.

# Design

- Share one storage-name list in the Rust edition. Use it in the status census, in the create reserved-name check (in the nested layout, with Python's refusal text) and in both audit enumerations (as a direct child of the control directory).
- Add a differential case for each surface, Rust against Python, on a nested project that holds every storage directory.

# Acceptance Criteria

- Rust create refuses each storage name in the nested layout with Python's message and exit code, and allows it in the flat layout.
- Rust audit lists no storage directory as an unregistered slot, matching Python.
- The new differential cases fail on the base commit.

# Notes

Found by the consumer's review seat while reviewing #2449, as F6. Both gaps predate #2449.
