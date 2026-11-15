---
title: 'wrkslots-archive-forward-compat: old readers must tolerate newer archive shapes'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] An archived record written by a newer
wrkslots bricks an older pinned reader. Disposing of an absent
Eden validate row appends an archive record whose checkout path
uses the Eden layout (`<slot>/checkout` under flat layout, taught
to the validators by the Eden absent-row fix). An older build's
`_archive_record_from_obj` expects every archived checkout at the
Git layout path and refuses the whole archive load with
`archive state.records[0] checkout main path escaped its archived
slot` — so every `remove` (and every other archive-loading
command) on that registry refuses, for every slot, until the
reader is upgraded. Consumers pin old versions against shared
registries, so mixed-version registries are the normal case, not
an edge case. Found while landing #1568; issue only, not fixed
there.

# Design

Proposed rule, both halves:

- **Readers tolerate, monotonically.** Archived rows are history:
  validate containment and identity (the path is one of the
  historical slot candidates or a recorded per-VCS destination
  inside one, digests and generations agree), never exact-layout
  equality. The accepted candidate-layout set only ever grows;
  exact-layout checks stay on active rows, where the current
  writer controls the shape. Apply in both the Python and Rust
  archive validators, with a differential fixture replaying an
  archive written by each historical layout.
- **Writers wait one release.** A new archive shape (new VCS
  layout, new required field) may be emitted only after the
  reader-side acceptance has shipped, gated on the archive schema
  version; until then the writer dual-writes or refuses the new
  shape with a named remedy. A single record an old reader cannot
  parse must surface as a typed, slot-scoped inconsistency for
  that record's slot, never as a whole-registry refusal for
  unrelated slots — corrupt-state fail-closed behaviour for the
  affected slot is unchanged.

# Acceptance Criteria

- Fixture: an archive containing a flat-layout Eden checkout
  record loads under the pre-Eden validator rules (tolerant
  reader) and unrelated slots still remove.
- A genuinely escaped path (outside every historical candidate)
  still refuses, by name, for its own slot.
- Python and Rust validators agree on every historical fixture
  (archive differential covers each layout era).
- The rule is written where archive writers will find it (the
  archive validator and the user guide's compatibility notes).

# Notes

Related: #1183 (archived rows keep created paths), the Eden
absent-row disposal that first emitted the shape, #1568 report
cross-version section.

## Resolution

[watermelon-full-preview] Reader-side tolerance landed (mu-archivefix): both archive
validators quarantine one contained record in a shape they do not know (unknown VCS, or a
non-Git checkout at a contained but unknown layout) and report it as `quarantined_records`;
the rest of the archive loads. A Git checkout outside its slot still refuses by name.
Writers stay strict (a candidate the reader would quarantine refuses before append).
Exact edenimg-t1 shape replay verified in Python and Rust; differential covers quarantine
and escape. Writers-wait rule recorded in the user guide.
