---
title: 'merge-queue-kept-greens-prefix: kept greens re-validate after the first one lands and moves the base'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T14:19:08.000000000+00:00
updated_at: 2026-11-07T14:19:08.000000000+00:00
---

# Description

Greens kept from a stopped run carry into the next run only while the
base is unchanged. When the first kept green lands, the base moves, so
every remaining kept green is re-planned as fresh work and re-validated
from scratch. In the reported run (2026-11-07), a new run landed the
first kept green on the unmoved base, then re-planned four more kept
greens as fresh work: about 20 minutes of re-validation, with a fresh
flake risk on work that had already been validated green. This caps
the landing cadence of a consuming project's queue after every stop.

# Design

- After a kept green lands, check the remaining kept greens in
  sequence. A kept green still lands without re-validation when what
  has landed since its base is exactly its validated prefix: the merge
  result equals the validated tree, so the tree that was validated is
  the tree that lands.
- Anything else is re-validated as today. In particular there is no
  disjoint-root landing on a moved base: a rooted batch is never landed
  on a base other than its parent (the stale-root-green invariant),
  because that merged tree was never validated.
- Withdrawn kept greens are dropped, not landed (the withdrawn-batch
  handling already on main); a kept green whose prefix includes a
  withdrawn change is re-validated or set aside by the same rules as
  any other non-matching prefix.

# Acceptance Criteria

- Kept greens v7..v12 with a stop: v7 lands, and v8 (whose validated
  prefix is exactly v7) lands without re-validation.
- A kept green whose landed-since-base history is not exactly its
  validated prefix — disjoint or overlapping, for example a disjoint
  v10 behind a red v9 — is re-validated, as today.
- A withdrawn kept green never lands from its kept green.
- Existing merge-queue behaviour is otherwise unchanged: greens kept
  while the base is unmoved, stale roots, refusals and culprit
  searches behave as before.

# Notes

Reported 2026-11-07 by a consuming project's merge queue; consumers
are referred to generically here, per repository convention. Rule (1)
only (exact validated prefix); the disjoint-root variant was
considered and dropped because it would land a tree that was never
validated.
