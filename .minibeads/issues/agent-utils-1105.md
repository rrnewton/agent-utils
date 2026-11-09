---
title: 'merge-queue-claim-revision: refuse a donor claim whose merge-queue revision differs from the queue host'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T01:22:18.000000000+00:00
updated_at: 2026-11-09T10:33:00.000000000+00:00
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

Landed on main as 481c57791b0a463b1ee9e1ff6ea1cac3f8917673 (gated commit 4aca6c96, files byte-identical after replay).

`donor claim` declares the donor binary's own revision via
`--revision COMMIT [--revision-dirty]`; the queue host refuses
a mismatched or unusable declaration before noting or leasing
anything, through `claim_refusal` around `revision_mismatch`
(same wording as the receipt path), replying
`{"job": null, "refused": "..."}` on an additive
`Claimed.refused` field that old and new binaries both parse.
The donor run loop declares on every claim and, on a refusal,
logs it, backs off like a tool mismatch and does not read it
as an empty queue. A claim with no declaration (an older donor)
claims exactly as before; a new donor against an older queue
leases nothing on its refused claim, as with `--drain` and
`--reports-admission` before it. Tests: `claim_refusal` admits
only undeclared or matching revisions (donor.rs unit), and an
end-to-end sequence — old-style claim leases, mismatched
declaration refused with the job still queued, matching
declaration (dirty test build declared dirty) leases
(cli_donor) — both failing on the old code first. Gate: full
gate on 4aca6c96, shared phase 239 passed / 0 failed
(rust.merge-queue.test 605/0); the exclusive cross
differentials were stopped under the merge-queue-only waiver
(ledger failed_steps empty, verdict fail only from that stop).
