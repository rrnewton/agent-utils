---
title: 'journal-reader-extension-slot: journal readers accept only exact keys, so every new journal field is a flag day for older pins'
status: open
priority: 3
issue_type: feature
created_at: 2026-11-09T11:10:12.000000000+00:00
updated_at: 2026-11-09T11:10:12.000000000+00:00
---

# Description

Both editions validate wrkslots journals (finish, create, seal and others) with
exact key sets. Adding any field, as #1121 finish-journal-live-remover does with
a top-level owner, makes every older pin refuse the journal with a StateError.
While such a journal is outstanding, an older pin sharing the registry fails on
every command. #1109 wrkslots-config-forward-compat solved the same problem for
the config file only (ignorable_keys plus min_wrkslots_version).

# Design

- Give journal readers a tolerant extension slot. Either:
  - a reserved object field (for example "x") whose contents older readers
    ignore; or
  - a listed set of ignorable keys, as #1109 does for config.
  New optional fields that older readers may safely ignore go there. A field
  older readers must not ignore keeps the exact-key refusal, and says so.
- Add a format or minimum-version marker so a reader can refuse clearly
  ("written by wrkslots >= X") rather than with a generic StateError.
- Both editions in one landing, with differential tests. The extension slot
  itself is a one-time flag day, so name it in the user guide's upgrade notes.

# Acceptance Criteria

- An older reader (post-change) ignores unknown fields in the extension slot,
  and still refuses unknown top-level keys.
- A field added through the slot is not a flag day: shown with a test that
  writes it and reads it with the reader before the field existed.
- User-guide upgrade notes updated.

# Notes

Follow-up to #1121, requested by the lead coordinator 2026-11-09. Filed by the
devbig030 coordinator.
