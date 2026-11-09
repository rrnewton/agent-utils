---
title: 'audit-dot-entries-not-slots: audit reports dot directories in the slots root as unregistered slots'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-09T03:50:00.000000000+00:00
updated_at: 2026-11-09T03:50:00.000000000+00:00
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
