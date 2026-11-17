---
title: 'wrkslots-dead-fuse-entry-refuses-host-wide: a dead FUSE entry in one slot refuses every other slot'
status: open
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T16:25:58-05:00
updated_at: 2026-11-17T16:25:58-05:00
---

# Description

[watermelon-full-preview] A pre-#1730 Eden validate slot kept its
FUSE mount inside the slot; a failed remove left dead FUSE copies
(stat fails with Errno 107, "Transport endpoint is not connected",
or EIO). Agent sandboxes run in private mount namespaces holding
slave copies of the mount table, so the dead copies persist per
namespace. Every `heartbeat`, `create` and
`classify-create-journals` whose namespace sees one such dead entry
refuses host-wide, although the entry belongs to another slot:
loading the global state stats every recorded checkout path, and
the registry storage census stats every slot and checkout, so one
unreachable mount point raises before any per-slot scoping applies.

# Design

- A stat failure with ENOTCONN (107) or EIO at or below a
  `fuse*` mount point listed in this process's mount table is a
  dead mount: scans record it and skip it, never raise, in both
  editions (Python `wrkslots`, Rust `wrkslotsd`).
- The entry is reported as a finding, `dead FUSE entry at <path>,
  owning slot <slot or unknown>`, row-scoped to its owning slot in
  the registry storage census (the way audit reports findings),
  and printed as a warning by heartbeat, create and
  classify-create-journals while they proceed.
- A command refuses, naming the entry, only when the dead entry
  lies inside the slot it acts on (or holds that slot, or is the
  path a create must write).
- A create journal whose own slot holds a dead entry is skipped
  by another slot's classification (the #1436 shape) and refuses
  its own slot's classification, naming the entry.
- Eden remove is out of scope (owned by the Eden remove lane).

# Acceptance Criteria

- With a fake mountinfo table plus a stat raising ENOTCONN (and
  EIO) for a path in another slot, `heartbeat`, `create` and
  `classify-create-journals` proceed and report the finding; on
  the parent code all three refuse host-wide.
- With the dead entry inside the acted-on slot (heartbeat /
  classify target, create target, or the classified journal's own
  slot), the command refuses, naming the entry, and writes nothing.
- The registry storage census reports a `dead-fuse-entry` finding
  scoped to the owning slot.
- Deterministic regression tests in both editions (fake mountinfo
  plus injected stat failures; no real FUSE mount, no wall-clock
  budgets).

# Notes
