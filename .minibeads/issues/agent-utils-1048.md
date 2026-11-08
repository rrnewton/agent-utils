---
title: 'merge-queue-terminal-verdict: a completed validation keeps its verdict when only cleanup fails'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-08T07:55:54.000000000+00:00
updated_at: 2026-11-08T07:55:54.000000000+00:00
---

# Description

Exit 75 (`REFUSAL_EXIT`) is read unconditionally as "validation
did not run": `is_refusal` (rs/merge-queue/src/validate.rs)
returns true for exit 75 before looking at anything else, the
parsed results are then wiped, `Ran::green()` additionally
requires exit 0, and a donor receipt with `refused` or exit 75 is
requeued with no verdict (`Store::report`). A consuming queue
lost a 23-minute green validation this way: its validator
printed its own terminal PASSED line, then its wrapper exited 75
because checkout cleanup refused (a deferred unit still
running). The queue logged "reported that validation could not
run; no verdict, queued again" and revalidated from scratch.
merge-queue has no terminal-verdict concept today: the only
verdict evidence is the results stream gated by the exit status,
and every marker it honours is likewise gated on its exact exit
(GREEN_FLAKY on 0, INFRA-BLOCKED on 6, RERUN-EXEMPT on red exits
excluding 75). Follow-up to #1045 merge-queue-donor-tool-identity,
whose analysis (relayed to the consuming queue 2026-11-07) found
a narrow sound form of rescuing such a run.

# Design

A completed run keeps its verdict when only post-verdict
housekeeping failed. All of the following, or the run stays a
refusal exactly as today:

- An explicit terminal claim printed by the validator itself
  (new contract, name to agree with consumers, e.g.
  `MQ_TERMINAL_VERDICT: PASS|FAIL`; do not adopt a
  consumer-private line name), or an early-verdict-style file
  carrying the same. Print/write it when the verdict is final,
  before cleanup.
- Recorded results COMPLETE for the requested selection: every
  selected test has a recorded outcome, judged on the rows as
  parsed, before `require()` synthesizes errors for the
  unreported (dagrun kind: journal completeness, same rule). A
  mid-run crash therefore cannot rescue itself; only a finished
  run can.
- No refusal signals: no COULD_NOT_RUN first line, no
  could-not-run scope lines. The claim agrees with the recorded
  results (PASS exactly when every recorded outcome is
  non-blocking); a claim over a recorded failure refuses the
  run as could-not-run, never lands it.
- Start with exit 75 only (the observed case). The exit status
  is still recorded on `Ran` and the receipt; `refused` becomes
  false only when the terminal verdict is accepted, and
  `green()` stops requiring exit 0 for exactly that case.
- #1045 attestation applies in full: a rescued verdict is a ran
  validation, so the refused-receipt exemption in
  `Store::tool_check` must NOT cover it — missing or mismatched
  `merge-queue-tool-identity` lines refuse it as
  identity-mismatch like any other receipt.
- A rescued verdict costs no could-not-run strike against the
  job or the donor (it is not a refusal; counting it would
  punish exactly the successful runs this rescues), and the run
  is never probed as a host fault for it.
- Local (non-donor) validations behave identically: the rule
  lives in validate.rs, donors only surface it. The cheaper
  consumer-side alternative stays documented in the guide: exit
  with the validation's own status and defer cleanup failure to
  a warning, never spend exit 75 after a verdict.

# Acceptance Criteria

- A command validator that writes complete passing results,
  prints the terminal PASS claim and the #1045 identity lines,
  then exits 75, lands green locally and by donor receipt, with
  exit 75 recorded and no could-not-run strike.
- Exit 75 with incomplete or absent results, with a refusal
  signal, or with a PASS claim over a recorded failure is a
  refusal exactly as today; a terminal FAIL over complete
  failing results is red, attributed, not a refusal.
- A rescued receipt without valid #1045 attestation is refused
  as identity-mismatch; a true admission refusal needs none.
- Existing refusal, could-not-run counting and probe tests keep
  passing; the new behaviour is covered for both validator
  kinds.

# Notes

Sketched by mu-archive during #1045 at the coordinator's
request after the consuming queue's 2026-11-08 incident (queue
run 20261108T055032Z, donor job
20261108T055718871-1c04259abf5a-1fd11a94 in their numbering);
filed for a lane decision, not yet implemented. The soundness
conditions above are the relayed analysis verbatim in substance:
a printed line alone is not evidence — completeness, agreement
and attestation are what let the exit code be ignored.
