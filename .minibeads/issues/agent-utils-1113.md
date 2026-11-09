---
title: 'audit-dot-entries-not-slots: audit reports dot directories in the slots root as unregistered slots'
status: closed
priority: 3
issue_type: bug
created_at: 2026-11-09T03:50:00.000000000+00:00
updated_at: 2026-11-09T05:30:00.000000000+00:00
closed_at: 2026-11-09T05:30:00.000000000+00:00
---

# Description

`_cmd_audit` (py/wrkslots/cli.py, around the current_on_disk construction) lists slots-root entries filtered only by is-dir, not-symlink, not-EVENTS and not-representation. A dot directory such as `.pytest_cache` is therefore reported as an unregistered slot ("not a Git checkout root"). Slot names must match `NAME_RE` (first character alphanumeric), so such an entry can never be a slot.

# Design

- Entries whose names do not match `NAME_RE` are not slot candidates.
- Don't drop them silently: report them once, in a separate "non-slot entries in the slots root" section (name and size, human and JSON), so stray junk stays visible.
- Mirror it in the Rust audit.

# Acceptance Criteria

- Test: a slots root with `.pytest_cache` (and another non-matching name) yields no unregistered-slot row, and one non-slot-entries listing with name and size, in both editions.
- Real unregistered directories with valid names are still reported as today.

# Notes

Reported by the lead (devbig014), 2026-11-08, from another team's ops lane. Low priority; can be batched with #1107 audit-format-label-mismatch (audit output).

## Resolution

Delivered by cl-wrkid, 2026-11-08 ET, in both editions, batched with #1107.

- Only directories whose names match `NAME_RE` are slot candidates (Python `_audit_slot_root_listing`; Rust: the same filter in both on-disk passes of `parse_audit`). Any other directory in a slots root, such as `.pytest_cache`, `_scratch` or a removal's `.<slot>.fenced.*` tree, no longer becomes an unregistered-slot row.
- It is listed once: JSON `non_slot_entries`, sorted by slot type and name, each with `slot_type`, `name`, `path` and `bytes`. `bytes` is allocated bytes with no symlink following and no device crossing, or null, meaning unknown, past 100000 entries or on a read error. Text output gets a section headed `non-slot entries in the slots root (no slot can have these names; inspect or delete them):`, one `  <type> <name>: <n> bytes` line per entry.
- Test: `test_audit_non_slot_entries_match_python`. A slots root holding `.pytest_cache`, `_scratch` and a valid-named `stray` directory gives no row for the first two and one listing with both, identical in both editions (JSON and the text section). `stray` is still reported as an unregistered slot, as before. The existing unregistered-directory differentials still pass.
- User guide: paragraph beside `audit --gate`.
