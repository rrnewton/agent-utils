---
title: 'merge-queue-claim-revision: refuse a donor claim whose merge-queue revision differs from the queue host'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T01:22:18.000000000+00:00
updated_at: 2026-11-09T01:22:18.000000000+00:00
---

# Description

A donor's merge-queue revision is compared with the queue host's
only after it has leased and run a job: the receipt path checks
tool identity (`revision_mismatch` in donor.rs) and refuses the
verdict, so a stale donor burns a full validation whose result is
then thrown away, claim after claim. The claim itself carries no
revision, so the queue cannot decline the lease up front.

# Design

- `donor claim` gains `--revision COMMIT` and `--revision-dirty`:
  the donor declares the revision its own binary was built from
  (`build_info::revision()` on the donor side, sent only when the
  build recorded one). Two flags rather than one `--revision
  <describe>` so the queue reconstructs the `Revision` exactly
  instead of parsing display text.
- The queue-host claim handler checks a declared revision first,
  before the probe-pass note and any leasing, via a small
  `claim_refusal(queue, declared)` helper around
  `revision_mismatch`: on mismatch (or an unusable declared
  revision, with the same text the receipt path produces) it
  leases nothing, writes no job, and replies
  `{"job": null, "refused": "<reason>"}` with exit 0 (a claim poll
  must not crash the donor loop). `Claimed` gains the additive
  `refused` field (serde default, omitted when empty), so old
  donors parse new replies and new donors parse old ones.
- The donor run loop sends its revision with every claim and, on
  a refused reply, logs the reason and backs off exactly as for
  a tool mismatch (it will be refused every claim until the
  binaries match, so it must not spin or exit as if idle-mistaken:
  the refusal counts as a failed claim, not an empty queue).
- Backward compatible where it can be: a claim with no revision
  declared (an older donor) claims exactly as today — the queue
  never invents a mismatch for an absent declaration. A new
  donor claiming from an older queue binary gets an
  unknown-argument refusal and leases nothing, the accepted
  pattern of `--drain` and `--reports-admission` before it; the
  donor and queue binaries ship together.
- User guide: the donor `claim` row gains the flags and the
  refused reply; the tool-identity discussion names the
  claim-time gate beside the receipt check.

# Acceptance Criteria

- A mismatched declared revision: no lease is written, the reply
  carries the `revision_mismatch` reason, and the donor claims
  normally once its binary matches.
- A matching declared revision claims exactly as today.
- No revision declared (old donor): claims exactly as today.
- A dirty or otherwise unusable declared revision is refused
  with the unusable text, as on the receipt path.
- Existing donor suites keep passing; the whole merge-queue
  crate stays clean.

# Notes

Fourth in the intake/donor chain after #1087
merge-queue-stranded-intake, #1103
merge-queue-landed-intake-cleanup and #1104
merge-queue-operator-hold. The donor tool-identity line item
belongs to another lane; this issue only adds the claim-time
revision gate. Tests: a `claim_refusal` unit test in donor.rs
plus cli_donor claim coverage.

## Resolution
