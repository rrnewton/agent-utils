---
title: 'wrkslotsd-observer-schema-writer-skew: observer rejects handoff.discarded_unaccounted'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-14T08:06:48.000000000+00:00
updated_at: 2026-11-14T08:06:48.000000000+00:00
---

# Description

[watermelon-full-preview] The wrkslotsd observer rebuild fails on every run against a
current event log: `active-state-recorded record.handoff has invalid fields: missing [];
unknown ["discarded_unaccounted"]`. The writer added the optional handoff field
`discarded_unaccounted` in c88e65a5 (#1364 wrkslots-finish-commit-accounting); the
writer/reader schema (`rs/wrkslotsd/src/active_state.rs` `handoff_from_obj` and
`py/wrkslots/cli.py` `_handoff_from_obj`) accepts it as an optional list of 40-hex
SHAs, but the observer schema (`rs/wrkslotsd/src/schema.rs` `validate_handoff`)
still requires exactly the four original keys. This is the second skew of this
kind in two days: 5961820b fixed `provisioned` in the same observer schema.

# Design

- `validate_handoff` accepts the optional `discarded_unaccounted`, a list of
  40-hex SHAs, validated the same way `active_state.rs` validates it.
- Stop this class from recurring: one shared table of the required and optional
  key sets for each record both schemas validate (active record, archive
  record, identity, handoff, checkouts, import source, task scope), used by
  both `schema.rs` and `active_state.rs`, plus a test that fails when the
  observer schema, the Rust writer/reader schema, and the Python writer in
  `py/wrkslots/cli.py` disagree on those key sets.

# Acceptance Criteria

- A handoff carrying a valid `discarded_unaccounted` list rebuilds; an invalid
  SHA in it still refuses, as do unknown or missing handoff keys.
- The drift test fails if any of the three schemas changes a shared record's
  required or optional keys without the others.
- End-to-end: the deployed observer binary fails the rebuild and the fixed
  build succeeds against the same read-only event log.

# Notes

[watermelon-full-preview] Number assigned by coordb. Reported after the observer
service failed every run since 2026-11-14 00:00 PST.
