---
title: 'journal-reader-extension-slot: journal readers accept only exact keys, so every new journal field is a flag day for older pins'
status: closed
priority: 2
issue_type: feature
assignee: mu-createlock
created_at: 2026-11-09T11:10:12.000000000+00:00
updated_at: 2026-11-09T13:28:11.000000000+00:00
---

# Description

Both editions validate wrkslots journals (finish, create, seal and others) with
exact key sets. Adding any field, as #1121 finish-journal-live-remover does with
a top-level owner, makes every older pin refuse the journal with a StateError.
While such a journal is outstanding, an older pin sharing the registry fails on
every command. #1109 wrkslots-config-forward-compat solved the same problem for
the config file only (ignorable_keys plus min_wrkslots_version).

# Design

Approved design (the lead, 2026-11-09, via the devbig030 coordinator):
the reserved object field `x`, not an ignorable-keys list. Journals are
machine-written, so the typo protection a hand-authored `ignorable_keys`
list buys for config (#1109) buys nothing here, while a writer that ships
a new field and forgets its list entry recreates the flag day this issue
exists to remove.

- `x`, when present, must be a JSON object, at most 16 KiB in canonical
  encoding (every progress write duplicates the journal into the
  append-only event log, where it is replayed and re-hashed forever).
  Contents are opaque to a reader that does not know a given subfield:
  never exact-keyed, and never stripped — stripping would break
  provenance, which compares the standalone journal file with the copy
  embedded in `operation-progress-recorded` events by canonical-JSON
  equality, and the embedded copy is inside the hash chain. This is the
  deliberate difference from #1109, whose config reader drops ignored keys.
- The rule, to be written into the user guide and the code comment beside
  the slot: **a field under `x` may only carry information an older reader
  can safely ignore. Anything safety-relevant must also bump
  `x.min_wrkslots_version`, so old readers refuse loudly instead of
  misclassifying.** Example: #1123's creator identity, if an older reader
  must treat a live creator as busy rather than as an interrupted create.
- `x.min_wrkslots_version` is the one reserved subkey (strict
  MAJOR.MINOR.PATCH, the #1109 grammar). Every top-level journal validator
  checks it before its exact-key check — recovery input validators and the
  separate validate-batch-seal validator included — and refuses naming the
  required and the reader's versions, mirroring the config message. A
  malformed marker refuses. A must-not-ignore field either raises the
  marker to its introducing release (which bumps `__version__` /
  `WRKSLOTS_VERSION`) or stays top-level and keeps the exact-key refusal.
- Unknown top-level keys still refuse. Unknown journal kinds still refuse.
  Nested objects (records, checkouts, authorizations, proofs) stay closed;
  a nested extension needs its own slot later. No warning is printed for
  an ignored slot: classification reads the same journal repeatedly on hot
  paths, and `status` already surfaces unreadable journals as findings.
- No writer emits `x` or the marker in this landing. The slot itself is
  the one-time flag day: pre-change pins refuse the `x` key. The 0.9.0
  version bump (both editions' version constants; #1122 already took
  0.8.0) marks the boundary:
  upgrade every pin sharing a registry to 0.9.0 before any writer emits
  `x`. The user guide's upgrade notes say so, including that a hand-restored
  quarantined journal carrying `x` fails the same way on an old host.
- Scope is every journal validator in both editions: all kinds dispatched
  by `_validate_journal_shape` / `validate_journal_shape` (create,
  import-existing, finish, legacy-validate-remove, the ownerless family,
  the recover-absent family), every duplicate recover/input
  re-validation, and the validate-batch-seal validator, which lives
  outside the shape function in both editions and is the easiest miss.
  `legacy-validate-remove` (no writer) still reads history with the slot;
  ghost kind `remove-validate-batch` (no writer, no validator) stays
  excluded. Best-effort readers that validate nothing today (journal
  enumeration, doctor listings, `--all-kinds`) are unchanged.
- One helper per edition runs the forward-compatible pre-check
  (marker, then `x` type/size) before each top-level exact-key check; `x`
  joins each optional key set. Rust has three `exact_keys` copies; only
  the journal path's caller changes — the copy serving ACTIVE/archive and
  event payloads must not grow journal tolerance.
- Writers: phase rewrites are mostly `dict(raw)` mutations and carry `x`
  for free; the roll-forward copy is verbatim. The one rebuilding writer,
  `_create_journal_payload` (and Rust create_write.rs), must thread its
  extension explicitly when Phase 2 adds the creator, or the field
  silently vanishes mid-operation. #1121's top-level finish `owner`
  lands before this change and is not moved into `x`.

# Acceptance Criteria

- An older reader (post-change) ignores unknown fields in the extension slot,
  and still refuses unknown top-level keys.
- A field added through the slot is not a flag day: shown with a test that
  writes it and reads it with the reader before the field existed.
- User-guide upgrade notes updated.
- Required tests (the lead): `x` not an object refuses; `x` over the cap
  refuses; an unknown key inside `x` is accepted by both editions; a
  minimum version above the current one refuses before the key checks.
  Differential in both editions, including the event-embedded journal copy
  so provenance equality with `x` present is covered.
- The implementation SHA goes to the coordinator for mu-rswrk2's review of
  the Rust half before landing.

# Notes

Follow-up to #1121, requested by the lead coordinator 2026-11-09. Filed by the
devbig030 coordinator.

Raised to P2 on 2026-11-09 by the lead coordinator: #1123's approved design puts its CREATE-journal creator identity in this extension slot, so #1124 lands first (no second flag day). Owner: mu-createlock, both editions in one landing; mu-rswrk2 reviews the Rust half before landing.

## Resolution

Delivered by mu-createlock in both editions: every top-level journal
validator — the shape functions, the duplicate recovery/input
re-validations, and the validate-batch-seal validators — now accepts a
top-level `x` object before its exact-key check. `x` is capped at 16384
canonical bytes and is never stripped on rewrite, so provenance equality
with the event-embedded copy holds. The reserved subkey
`x.min_wrkslots_version` refuses naming both versions ahead of any key
error, mirroring #1109's config order; unknown top-level keys still
refuse, and nested objects stay closed. No writer emits `x` yet.
wrkslots 0.9.0 in both editions is the one flag day; the user guide's
upgrade notes carry the rule that a field under `x` may only hold
information an older reader can safely ignore, and anything
safety-relevant must raise the minimum version instead. mu-rswrk2
reviewed the Rust half and caught the duplicate ownerless-clone
validator in recover_clone_write.rs, which gained the same pre-check.
Required differential tests: an unknown key inside `x` is accepted by
both editions (the recover-clone resume path included), `x` not an
object and over the cap refuse identically, and a newer minimum gives
the version refusal before key checks.
