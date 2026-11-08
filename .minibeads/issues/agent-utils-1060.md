---
title: 'merge-queue-recount-refusals: recount names only the first undeclared decrease and refuses every pin removal'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-08T13:11:07.000000000+00:00
updated_at: 2026-11-08T16:47:43.000000000+00:00
---

# Description

Two recount refusals cost one queue round trip per pin, and one of
them makes a deliberate change impossible:

1. `check_decreases` in `rs/merge-queue/src/recount.rs` returns at
   the first decreasing pin that lacks an exact
   `Count-Pin-Decrease` declaration. A change that lowers several
   pins therefore costs one run per pin: the first run names one
   missing trailer, the author adds it, the next run names the
   next, and so on. A consuming project's queue hit this with one
   change lowering three pins by two each; two runs each refused
   naming only one pin and a third accepted once all three
   trailers were present (~10 minutes of avoidable round trips).

2. `change_deltas` refuses any pin whose occurrence count changes
   ("occurrence-count mismatch for pin ...: base 1, merge base 1,
   head 0"), so a deliberate pin *removal* cannot land through the
   queue at all (short of `--exact`). A consuming project is
   replacing count pins with census gating and needs to delete
   dozens of pin entries in one change; there is currently no way
   to declare that intent.

# Design

All-at-once refusals:

- Collect every decreasing pin that is undeclared (and every
  declaration whose amount does not match) before returning, in
  the delta map's stable `(name, occurrence)` order, and return
  one refusal naming all of them. Each undeclared decrease gets a
  ready-to-paste trailer line of the existing form
  `Count-Pin-Decrease: <pin> <amount> <reason>` (with `name:N` for
  a nonzero occurrence), so the author can add the whole block in
  one edit.
- Decreases declared with exactly the right amount are honoured
  and are not listed in the refusal.
- Preserve the existing single-pin refusal wording/shape when
  there is exactly one problem, including naming the merge-base
  and head values and any unreadable trailer lines.
- Do not change what counts as an accepted decrease: exact pin,
  exact occurrence, exact amount, non-empty reason.

Declared removals:

- A new head-commit trailer declares an intended removal:
  `Count-Pin-Removed: <pin> <reason>`, one per pin name, or a glob
  with an expected removed-entry count, e.g.
  `Count-Pin-Removed: privileged-* 56 census gating`. The count,
  when present, must match exactly; a glob covers only names it
  matches.
- A name whose occurrences all disappear at the head
  (`merge base >= 1`, `head == 0`) is accepted when a removal
  declaration covers it, and refused as today otherwise. The
  refusal follows the all-at-once rule above: one refusal listing
  every undeclared removal, each with a ready-to-paste
  `Count-Pin-Removed:` line.
- Partial occurrence-count shrinks of a repeated name
  (`head >= 1`) stay refused even when declared: occurrence
  identity is positional, so which occurrence was removed is
  ambiguous. Removing a whole name is the supported shape.
- Occurrence-count *increases* are decided explicitly and stay
  refused for existing names: a wholly new name (absent at base
  and merge base) already takes the head's values, but added
  occurrences of an existing name are ambiguous — occurrence
  identity is positional, so an append cannot be told from an
  insert ahead of the others — and any count change where the
  base also changed the count since the merge base is likewise
  ambiguous. The user guide spells the whole table out. (This
  asymmetry with whole-name removals is deliberate: a vanished
  name leaves no identity question.)
- A removal declaration is recorded like a decrease declaration
  (run report and item history), so a landed removal is auditable
  after the fact.

No per-repo recount config file: declarations stay in trailers
(reviewable with the change that needs them — a policy file the
same change could edit would let it authorise itself) plus the
existing queue config. If a second repository ever needs a
different pin file location than the compiled-in `PIN_PATH`, that
belongs in the queue config, not in a file read from the
candidate tree.

# Acceptance Criteria

- A head with three undeclared decreases gets one refusal naming
  all three pins in stable order, with three trailer lines
  carrying the correct pin/occurrence and amount for each.
- A correctly declared decrease in the same head is not listed in
  that refusal.
- The existing single-pin refusals keep the same wording/shape
  (values named, exact-amount rule, unreadable trailers named).
- A head removing several pins with covering `Count-Pin-Removed`
  declarations (including one glob with an exact count) lands;
  the same head without declarations gets one refusal listing
  every undeclared removal with a ready-to-paste trailer each.
- A partial occurrence-count shrink is still refused, declared
  or not; added occurrences of an existing name are still
  refused, as is any count the base also changed.
- Existing recount unit and CLI tests keep passing.

# Notes

Reported by a consuming project's merge queue (two reports: the
one-pin-at-a-time decrease refusals, and a deliberate mass pin
removal blocked on the occurrence-count refusal); consumers are
referred to generically here, per repository convention. Queued
behind #1053 merge-queue-rebase-conflict-probe. Gate as a product
change under the merge-queue rules (full shared gate phase;
exclusive phase waived for a merge-queue-only diff).

## Resolution

Landed on main as 7e13f97fb48d7f7a7896c29cbb696402192bdb34 (commits gated recount commit d3ecddaf replayed as 7e13f97f, in the same push as plan-intake d3f107ad).

`check_decreases` collects every problem — undeclared decreases and
amount mismatches — into one refusal in delta order, each undeclared
decrease with its ready-to-paste trailer; the single-problem wording
is unchanged. Whole-name removals are accepted with a
`Count-Pin-Removed:` trailer (exact name with optional entry count,
or a glob whose exact removed-entry count must match), honoured
removals are recorded like decreases (`count-pin-removed`,
`pin_removals`), and undeclared removals are all named in one
refusal. Partial occurrence-count shrinks and added occurrences of
an existing name stay refused (positional identity); a count the
base also changed since the merge base stays ambiguous and refused.
Covered by recount unit tests (trailer parsing, glob honours,
all-at-once naming, count mismatch, partial shrink, append refusal)
and cli_recount tests (declared mass removal lands and is recorded;
undeclared mass removal names every pin). Gate: full shared phase on the combined stack passed clean (dagrun 238 passed / 0 failed; merge-queue step 563 passed / 0 failed, wall 525.437s at user 514.481 + sys 691.853 CPU-s under the raised 2400 budget); the gate-exclusive cross differentials were stopped under the coordinator waiver for this merge-queue-only diff; the combined replay onto the then-current main was patch-identical with fmt/clippy and the recount/disjoint/confirm/plan suites green.
