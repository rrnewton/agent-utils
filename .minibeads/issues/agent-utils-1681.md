---
title: 'wrkslots-remove-proof-fetch-cost: one advertised-refs proof per repository per removal'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-15T19:50:00+00:00
updated_at: 2026-11-15T19:50:00+00:00
---

# Description

[watermelon-full-preview] Slot removals with unpushed private
history (`remove --archive-private-history`) take tens of minutes.
Measured on a throwaway scratch slot (own project, six private
submodule branch tips on a published base): one removal runs 782
Git calls and 5 `ls-remote` round trips, ~98 s wall against a
local-file remote where each round trip is nearly free. Against a
network remote each of those round trips costs seconds, and the
count grows with every checkout and every salvage re-derivation.

Two multiplications cause it. `_thin_bundle_prerequisites`
(#1367) asks `remote_advertised_tips` for the remote's advertised
refs on every salvage attempt — prefetch and again under the
registry lock, per candidate — and that answer was never kept, so
each attempt paid a fresh `ls-remote`. The commits-only proof
fetch (`_fetched_proof_store`) already batches every missing name
into one `fetch --stdin`; it is not the multiplier. Separately, a
remove whose launching wrapper is killed keeps running: no signal
reaches the client inside its scope, and one remove ran 44
minutes past its parent's death.

# Design

- Keep the advertised-tips answer for one removal under the
  existing containment scope and age bound
  (`_REMOTE_CONTAINMENT_EVIDENCE_MAX_AGE_SECONDS`), keyed by
  object store and remote URL, refusals included — exactly the
  treatment `remote_heads_containing` already gets. One removal
  therefore asks each repository once, however many salvage
  attempts re-derive thin-bundle prerequisites from the answer.
  Outside a removal scope nothing is kept, as before.
- A remove-family client (`remove`, `remove-agent-batch`,
  `remove-validate-batch`, `retire-pending`, `finisher`) asks the
  kernel for a parent-death SIGTERM at startup (Linux `prctl`),
  so a killed wrapper stops the client with it. A process
  parented to PID 1 from the start is left alone.
- Safety properties kept, unchanged: the proof store still
  starts empty with no alternates and `GIT_NO_LAZY_FETCH`; the
  temporary proof repository still lives inside the checkout's
  own Git directory, never the system temporary directory; the
  300-second evidence age bound is unchanged, so no proof is
  trusted longer than before. The one property that changes is
  stated above: a remove client now terminates when its parent
  dies, where before it ran to completion.

# Acceptance Criteria

- A throwaway slot removal asks each repository for its
  advertised refs at most once per removal scope (measured:
  `ls-remote` 5 -> 4 on the six-tip fixture; the remaining four
  are the distinct containment questions of distinct
  repositories/phases, not repetitions of one question).
- Focused tests: advertised tips are memoized inside the scope
  and not outside it; a child that installed the parent-death
  signal dies when its parent dies.
- Two removes of different slots still validate in parallel:
  the network and proof phase runs before the registry lock
  (`_prefetch_remote_containment`, called before
  `_mutation_locks` in `_cmd_remove_in_scope`); only the record
  commit is serialized.

# Notes

[watermelon-full-preview] Measured before/after on the same
fixture; the proof fetch itself was already batched and fires
only when advertised tips are missing locally, so the win on a
network remote is one round trip per salvage re-derivation per
repository, plus the removal no longer outliving its wrapper.
