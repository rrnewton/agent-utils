---
title: 'merge-queue-recount-exact-retired: recount refuses exact changes and cannot accept deleting the pin file'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-08T16:55:02.000000000+00:00
updated_at: 2026-11-08T16:55:02.000000000+00:00
---

# Description

Two recount refusals from one consuming project's queue, both on a
change that deletes its whole count-pin table by design (dozens of
entries) as it replaces count pins with census gating:

1. The change was submitted `--exact --solo --prioritized`, yet
   intake still ran recount-pins and refused it with an
   occurrence-count mismatch (base 1, merge base 1, head 0 for the
   first deleted pin). The user guide's exact rule is that the
   queue never rewrites an exact change (pins, recount,
   regeneration — a regeneration that would change it ejects it);
   subjecting it to recount at all contradicts that contract.

2. Even without `--exact`, deleting the pin *file* wholesale
   cannot be declared. #1060's `Count-Pin-Removed` covers names
   whose entries disappear while the file remains, but a head
   with no pin file at all hits the older "pin file missing in a
   version" refusal in `change_deltas`, and there is no trailer
   that says "this table is retired". The only current escape is
   turning recount off for the whole queue.

# Design

- Exact changes skip recount entirely, on every path that would
  run it today (intake planning and candidate construction alike):
  no delta computation, no decrease check, no candidate rewrite,
  no recount refusal. The candidate is the submitted head, as
  exact already promises.
- A head with no pin file is accepted when its head commit
  declares the retirement with one trailer,
  `Count-Pins-Retired: <reason>`: the change recounts to nothing
  and the retirement is recorded like a removal (run report and
  item history). Without the trailer the refusal stands, and it
  names the trailer that would declare it. A pin file missing at
  the *base* or *merge base* while present at the head keeps
  refusing: there is no measured-from version to retire from.
- `Count-Pin-Removed` (per name or globbed) stays the
  partial-deletion path while the file remains; the retired
  trailer is only for the file going away entirely. If a head
  both deletes the file and carries per-pin removal trailers, the
  retirement declaration is the one that counts.
- User guide: state both rules in the recount section (exact
  skips recount; deleting the file needs the retirement trailer).

# Acceptance Criteria

- An `--exact` change that deletes the whole pin file lands
  with no recount refusal and no pin rewrite, recorded as exact.
- A non-exact change deleting the whole pin file with a
  `Count-Pins-Retired:` trailer lands, and the retirement is
  recorded in the run report and item history.
- The same change without the trailer is refused in one message
  naming the missing file and the trailer that would declare it.
- A pin file missing at base or merge base still refuses.
- Existing recount unit and CLI tests keep passing.

# Notes

Reported by a consuming project's merge queue; consumers are
referred to generically here, per repository convention. That
queue is unblocked meanwhile by disabling recount, at the cost of
losing it for every other change. Queued right after the recount
refusals / plan-intake stack lands, ahead of #1054
merge-queue-sync-exact. Gate as a product change under the
merge-queue rules (full shared gate phase; exclusive phase waived
for a merge-queue-only diff).
