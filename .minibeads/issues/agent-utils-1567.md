---
title: 'eden-backing-in-image: one Eden backing repo inside the worktrees image'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T19:00:00+00:00
updated_at: 2026-11-15T19:00:00+00:00
---

# Description

[watermelon-full-preview] The shared Eden backing repo lives on the host
home disk, so Eden metadata growth and exhaustion land on the host file
system. The owner wants ONE backing repo inside the worktrees image,
shared by every Eden slot checkout there, keeping that growth bounded
by the image (with the outer-only host cap of #1566).

# Design

- Placement: `<image mount>/.eden-backing/<name>`, its own subvolume
  (no per-slot quota), outside every slot, never removed with a slot
  (`_remove_slot_directory` refuses it; directory censuses and layout
  migration allowlist it explicitly).
- Selection: `configuration.worktrees_image.eden_backing_repo: <name>`.
  Create validates, never overrides: the given name or its resolved
  in-image path is accepted and normalised; anything else refuses
  before the create journal is written. Unseeded refuses naming the
  seed verb; create never seeds.
- Seed: `wrkslots worktrees-image seed-eden-backing --from <host
  backing>` (coordinator-run, dry run by default). Local clone only:
  `sl clone -U <source> <staging>` (this Sapling build has no
  `clone --pull` and no `pull --dry-run`), then standalone proofs:
  repo shape, no sharedpath, store on a different device than the
  source, default path reset to the source's own upstream, and a
  standalone `sl log` read. Atomic rename publishes. Reports carry
  source_bytes/files (exact dry-run measurement), dest_bytes and
  wall_seconds on --yes, lazy-store markers (a lazy store still
  fetches from its upstream on demand, like the host backing),
  image free bytes before/after, and the backing subvolume's qgroup
  referenced bytes. If the local clone cannot stand alone, stop: a
  network-clone fallback needs its own size/time estimate first.
- Concurrency: several per-slot daemons share the one store as they
  already share the host backing; no wrkslots store lock of its own.
  The store stays writable (fetches and commits append). Boxed lanes
  bind the in-image backing writable (blast radius: this project's
  image); a host backing is never bound writable.
- Remove/recover: unchanged Eden sequences; slot removal can never
  delete the backing. Interrupted seeds leave only
  `.seeding-<name>-*` staging (`--clean-staging` removes exactly
  those, under the mutation locks). No backing-delete verb in v1:
  deletion is manual after the last Eden slot is gone.
- Real seed target (per the consuming team): their project image at
  `.eden-backing/source-repo`, existing checkouts move at next recreate,
  host backing and checkouts untouched. The real seed runs only
  after the dry-run report is reviewed and the host is quiet.

# Acceptance Criteria

- Stubbed-runner tests cover settings accept/refuse, backing problem
  variants, seed argv + default rewrite, seed refusal when the clone
  stays pointed at its source, create refusal on mismatch and on
  unseeded backing before writing, and qgroup report parsing.
- Rust parity: settings parse/validate/normalise, config docs, and
  the status report's eden_backing entry match Python.
- Capped validate and gate green; no real Eden operation in tests.

# Notes

Resolution: (fill in at landing)
