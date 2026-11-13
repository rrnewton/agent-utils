---
title: 'merge-queue-speculative-stacking-default: validate each root stacked on the candidates ahead of it, and land by checked fast-forward'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-12T00:00:00.000000000+00:00
updated_at: 2026-11-12T00:00:00.000000000+00:00
---

# Description

Owner decision (2026-11-12, approving the noon proposal: "Feel free to
update it also."):

> (1) SPECULATIVE STACKING AS THE DEFAULT: validate each root on top of
> the candidates ahead of it, so a landing no longer cancels the roots
> behind it. (2) A CHECKED FAST-FORWARD of main: land a validated
> stacked candidate by fast-forwarding main only when main is exactly
> the base it was validated on (checked, never a blind push).

Context: at noon one landing cancelled 3 parallel validations in one
consuming queue, throwing away 2,626 donor-seconds plus 20-25 min to
re-run. Safety requirement: a stacked candidate whose base root FAILS
must be re-validated on the corrected base, or ejected; it is NEVER
landed on the strength of a validation that assumed the failed root.
The same invalidation covers a base root that turns STALE (an external
landing moves the base under the stack): every stacked flight above
re-plans onto the new base.

The default changes for every consuming queue: queues that do not opt
out validate stacked by default after this lands. A consuming queue
that is parked will not feel the change until it resumes.

# Acceptance Criteria

- With default config, a would-be root validates stacked on the tip of
  the candidates ahead of it (no disjoint-root cancellation on landing);
  `init --no-speculative-stacking` restores the previous disjoint-root
  behaviour and is recorded in the user guide.
- Landing uses a remote-side compare (push
  `--force-with-lease=<ref>:<validated parent>`, local old-value
  `update-ref`): main moves only when it is exactly the validated
  parent; a race between check and push refuses and re-plans.
- Stack of 3 all green lands in order via checked fast-forward; middle
  root fails, so the ones above re-validate or eject and nothing lands
  on the failed assumption (mutant-checked); main moved externally, so
  the fast-forward refuses and re-plans.
- Builds on the existing k-roots/`keep_stale_roots` and stacking
  machinery (#915), not a parallel path. Also closes #935
  merge-queue-stale-green-stack (subsumed and generalized) and #1036
  merge-queue-planner-stacking (its `--after` composition and
  prioritized stale-green rooting stay as explicit exceptions).

# Design

- `Config.speculative_stacking` (default true; old configs without the
  field load as true) gates the `Runner::pipeline` fill loop: a fresh
  batch that would root on the base instead builds on the stack top.
  The prioritized stale-green root exception and the `--after`
  dependency-parent wait are preserved.
- `Runner::land_with` push path lands with
  `--force-with-lease=<base-ref>:<candidate.parent>` (local driver:
  `update-ref` with the validated parent as old value), so the compare
  is atomic on the remote; a moved base returns `BaseMoved` and the
  batch re-plans, never a blind push or a retry into a moved base.
- Invalidation: a red or ejected head discards every stacked flight
  built on its tree and requeues them to re-validate on the corrected
  base; a stale stack bottom (its parent no longer the refreshed base)
  is discarded the same way, so no stacked green lands from a
  validation that assumed a failed or moved base root.

# Notes

Tests use a fake validator and wait on events, never on wall-clock
waits. Coordination: the pipeline fill loop and `Runner::land_with`
are owned by this change; validator-purpose, base-evidence, stop-text
and notice-registry hunks in the same crate are owned separately and
this change replays over them rather than rebasing.
