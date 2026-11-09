---
title: 'merge-queue-operator-hold: an operator host fault a probe pass cannot clear, and priority changes for changes already in flight'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T00:44:51.000000000+00:00
updated_at: 2026-11-09T10:33:00.000000000+00:00
---

# Description

Two operator controls are missing:

1. A host fault recorded by hand holds no longer than the donor's
   next passing probe. `donor host-fault` writes the same record
   shape as a probe failure, so `donor claim --probe-passed` (via
   `note_probe_pass`) or an admitted validation clears it again:
   an operator who holds a donor out for a reason the probe cannot
   see (a host being drained, a suspect toolchain, an
   investigation) watches it lease work again on its next claim
   cycle. There is no way to say "held until I clear it", and
   nothing in `donor status` distinguishes such a hold.

2. Priority is fixed at submit time. Reordering a queued or
   in-flight change means resubmitting it, which replaces the
   item (new submission record, head re-checked, refusal
   backoff reset when the head differs) to change one ordering
   field, and for a change in flight cannot take effect until
   the run gets around to the recorded resubmission.

# Design

Operator hold:

- `donor host-fault --operator` records `"operator": true` in the
  donor's `host-faults.json` entry (absent means false, so old
  files and mixed versions read as today). `note_host_fault`
  gains the flag; probe-failure recording passes false.
- `note_probe_pass` leaves an operator fault entirely alone: no
  clear for an ordinary one, no trial arm for an escalated one.
  Probe outcomes are still recorded on the entry
  (`record_probe_result`). The admitted-validation clear in
  `Store::report` likewise skips operator faults. Only
  `donor clear-fault` without `--probe-passed` lifts one;
  `clear-fault --probe-passed` on an operator fault says it is
  held by the operator and changes nothing.
- `host_fault_details` carries the flag; `donor status` renders
  an operator fault as `held by operator: <reason>` (with since
  and probe state as today), and the serve heartbeat includes
  the same marker. Claim behaviour otherwise unchanged: a held
  donor is leased nothing.

In-flight priority:

- New verb `prioritize <PR>` mirroring submit's two ordering
  knobs: bare `prioritize PR` sets `prioritized`; `--priority N`
  sets the numeric priority (lower lands first); `--clear`
  resets both (prioritized off, priority back to the default).
  With the writer lock free it edits the item in place with a
  history note (`ops::prioritize`); a landed change is refused.
- While a run holds the lock it is recorded as a new intake
  request `Prioritize { pr, priority: Option, prioritized:
  Option }` (None leaves that knob unchanged), like `Retry`.
  Applied by `intake::perform` through the same `ops` path; a
  change that landed meanwhile drops it with a note, as #1103
  does for submits. Old binaries that cannot read the new
  variant move it to `intake/refused` unreadable, as with any
  unknown record; new readers accept every existing entry.
- Unlike a resubmission, a waiting prioritize applies at the
  run's next intake point even for a change in flight (the
  withdrawal precedent in `take_intake`): it is removed from
  the busy set and `intake_unblocks` wakes the round for it.
  The in-flight batch is undisturbed; `plan_intake` then
  replans from the updated item records, so the next decision
  (pending order, and claim stamps on jobs published after it)
  uses the new priority in the same run.
- User guide: host-fault/clear-fault flag rows and status text,
  a `prioritize` section, and the intake section's list of
  requests that apply mid-run.

# Acceptance Criteria

- After `host-fault --operator`, a probe-passed claim leases
  nothing and the fault stays; a recorded probe pass and an
  admitted validation likewise leave it; `clear-fault
  --probe-passed` leaves it and says why; plain `clear-fault`
  lifts it and the donor claims again.
- `donor status` shows `held by operator: <reason>`.
- Ordinary (non-operator) faults behave exactly as today,
  escalated trial behaviour included.
- `prioritize` on a change whose batch is in flight leaves the
  batch undisturbed and the next plan in the same run orders
  by the new priority; with no run running it applies at once;
  `--clear` restores the defaults.
- Existing donor and intake suites keep passing; the whole
  merge-queue crate stays clean.

# Notes

Third in the intake/donor chain after #1087
merge-queue-stranded-intake and #1103
merge-queue-landed-intake-cleanup. Test homes: the host-fault
block in cli_donor plus donor.rs unit tests; prioritize in
cli_intake_window (gated in-flight batches) and cli_intake
(recorded/next-writer paths).

## Resolution

Landed on main as 63ad52e8ca298f56bba9161da21c6892a9af4ce9 (gated commit 1cb88128, files byte-identical after replay).

`donor host-fault --operator` records an operator hold in
host-faults.json: `note_probe_pass` leaves it entirely alone
(no clear, no escalated trial), an admitted validation does not
clear it, and only plain `donor clear-fault` lifts it —
`clear-fault --probe-passed` records the probe outcome, replies
`held_by_operator` and changes nothing. `host_fault_details`
and the `donor fault` reply carry the flag, and `donor status`
plus the serve heartbeat render `held by operator: <reason>`.
The new `prioritize` verb edits a queued change's numeric
priority and/or prioritized flag in place (`--clear` restores
the defaults; a landed change is refused), directly under the
lock or as a recorded `Prioritize` intake request that a run
applies at its next intake point even for a change in flight
(`apply_priorities` ahead of the busy-filtered apply, and a
waiting prioritization opens the intake round with a full
window), so the next plan orders by it without disturbing the
batch running. Tests: an operator fault surviving both probe
paths until cleared (donor.rs unit + cli_donor end-to-end incl.
status text and the fault JSON), prioritize in place and when
recorded (cli_intake), and prioritizing in flight reordering
the next plan (cli_intake_window), all written failing first.
Gate: full gate on 1cb88128 — shared 238 passed with one
failure in untouched timeline.browser.functional (dev-server
port already in use on the shared host), exclusive phase 7/7
passed, rust.merge-queue.test clean at 603 passed / 0 failed;
the timeline step passed its solo exclusive rerun (ledger
rerun_passed).
