---
title: 'merge-queue-donor-tool-identity: the donor runs the tree it identifies'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-08T04:13:13.000000000+00:00
updated_at: 2026-11-08T04:13:13.000000000+00:00
---

# Description

A donor answers `validator.tool_identity` before a run and the queue
verifies that answer on the receipt, but nothing binds the answer
to what `validator.run` actually executes. The canonical identity
command reads a ref (`git -C tool rev-parse origin/main:<path>`)
and the canonical refresh only fetches (`git -C tool fetch`), so
after a refresh the answer names the fresh `origin/main` tree
while the validator still loads the tool from a working tree
that was never moved. A consuming queue observed a donor run a
validator from a working tree 398 commits old while reporting
current `origin/main` identity; the receipt certified the pre-run
ref answer and the queue accepted it. The executed tree is
unattested end to end. Sibling of #1014 merge-queue-kept-greens-prefix
(the tree that was validated is the tree that lands) and builds on
#1005 merge-queue-donor-restamp-race descendant acceptance.

# Design

Format agreed with the consuming queue (2026-11-07): the
validator prints one line per tool from inside the run,
`merge-queue-tool-identity: <tool> <40-hex value>`, attesting the
tree it actually loaded.

- Both validator kinds parse those lines out of the run log into
  the run result; the donor records them on the receipt as a new
  field beside the pre-run answers (serde-default, so receipt/v1
  stays parse-compatible), preserving the pre-run vs in-run
  distinction as evidence.
- The queue requires, for every tool whose stamp is a full hex
  identity, a printed value for that tool that equals the stamp
  or provably descends from it (the #1005 proof, in the queue's
  own repository). A missing, malformed, extra, stale-ancestor or
  unrelated printed value refuses the receipt as
  identity-mismatch; the job stays leased and nothing lands on
  that receipt. Non-hex (version-string) stamps cannot descend:
  a printed line for such a tool must equal the stamp, and its
  absence keeps today's behaviour.
- USER_GUIDE ("Tool identity and receipts") and the
  `ValidatorConfig`/CLI docs gain the contract: tool_identity
  identifies exactly what validator.run executes, and
  tool_refresh updates that; a refresh that only fetches must
  also move whatever the validator loads.
- `tool_run_tree` (merge-queue materializing the identified
  tree for the validator) stays optional and is not built: the
  printed attestation closes the hole at the only place that
  knows what ran, without merge-queue staging tool checkouts.

# Acceptance Criteria

- A donor whose pre-run identity answers are fresh while its
  validator prints a stale ancestor tree gets its receipt
  refused as identity-mismatch and nothing lands (regression
  test fails on pre-fix code).
- Printed values equal to the stamp, and printed descendants of
  the stamp, are accepted; unrelated and stale-ancestor values
  are refused.
- Existing pre-claim, restamp and descendant-acceptance tests
  keep passing; duplicate/conflicting attestation lines never
  attest two trees at once.

# Notes

Reported 2026-11-07 by a consuming queue (donor observed 398
commits stale); print format agreed with it the same day. The
queue host's own local validator runs share the parsing but
enforcement is donor/receipt-side, where the attestation crosses
a trust boundary.
