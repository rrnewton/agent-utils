---
title: 'wrkslots-live-finish-journal-blocks-create: a live finish journal for one slot blocks create for another'
status: open
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T00:42:32.000000000+00:00
updated_at: 2026-11-17T00:42:32.000000000+00:00
---

# Description

Every single-remove finish journal on a shared host is about 4.0 MiB
(4,165,721 to 4,170,799 bytes over 4 removes). While any finish is live
(106-154 s each), a validate checkout create refuses, naming another
slot's journal as an oversized recovery journal exceeding the 1 MiB
safety bound, and advises recovering or discarding the named journal.
A live, healthy finish journal for another slot is therefore classified
as a stranded recovery journal, and the remedy tells operators to run
recover against a live operation.

The classification reads and size-checks every outstanding journal
before it knows the journal's kind or whether its recorded owner is
alive, so size alone decides. Separately, the finish journal embeds
the full remote-containment inventory twice per checkout (in the
recorded row and in the salvage receipt), which is what makes it
megabytes on a repository whose HEAD is contained in many remote refs.

# Design

- In create-journal classification, decide liveness first (recorded
  owner pid plus start time): a journal whose owner is alive is never
  a recovery journal. A live finish journal for a different slot whose
  paths do not overlap the create's targets does not block the create;
  an overlapping one stays busy, naming the live holder. Read and
  size-check a journal only when it is actually a recovery candidate
  (owner dead or unknown). Never suggest recover for a live journal.
- Shrink finish journals: containment inventories beyond a small
  inline limit move to a digest-referenced side file, written and
  fsynced before the journal that references it and removed only after
  the journal is gone. Recovery restores the inventory from the side
  file and refuses cleanly, naming the digest, when it is missing.

# Acceptance Criteria

- A live finish journal for another slot does not block create.
- A dead-owner oversized journal still refuses as before.
- A 4 MiB-class journal is classified without the recovery read when
  its owner is live.
- Python and Rust classification agree on all three cases.
- A finish journal with a bulky containment inventory stays small,
  and recovery with its side file present behaves as before; with the
  side file missing it refuses naming the digest.

# Notes

Part (a), the liveness-first classification, unblocks validation and
lands first. Part (b), the sidecar shrink, follows as its own change.
