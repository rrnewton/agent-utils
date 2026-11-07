---
title: 'merge-queue-confirm-probe-blocks-run: the same-commit confirm probe runs synchronously inside handle_red, so the whole run stops while the probe waits for a donor'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T05:15:00.000000000+00:00
updated_at: 2026-11-07T05:15:00.000000000+00:00
---

# Description

With `validator.confirm_reruns` on, a red single-change batch gets one
same-commit rerun of its failed tests (the probe) before the change is blamed.
The probe runs synchronously: `Runner::pipeline` calls `Runner::handle_red`,
which calls `Runner::validate_all` and joins the probe's scoped validation
thread. With `offload: decide` the probe is a donor job, and donors claim the
oldest queued job (`Store::claim` in donor.rs), so the probe waits behind every
job published before it. While the probe waits and then runs, the pipeline's
main loop does not iterate, so the run:

- takes in no recorded request (intake is taken only in the main loop, at a
  free window slot), not even a prioritized submit;
- decides no other batch: a green rooted batch that jumped over the red one
  stays undecided (`rooted_decidable`), and later reds wait for their own
  probes, one at a time;
- starts nothing new.

Measured on a consuming project's queue on 2026-11-07, run
20261107T044307Z, merge-queue 0c145199, 2 deciding donors at cap 1:

- 04:53:53Z: v1, a batch of one change, red. 04:54:54Z: its probe v6 is
  published as donor job 20261107T045454694, 23 ms after the job of batch v5.
- 05:01:44Z: v3 red. 05:05:05Z: v2 green in 1054 s; v2 is a rooted batch that
  jumped over v1, so it stays undecided. 05:09:10Z: v4 red. Neither red's probe
  had started.
- Intake: a prioritized submit recorded at 04:53:13Z and two more at 04:57:46Z
  and 05:07:11Z; none applied. `status` said "applied at its next free slot".
- One deciding donor was busy with v5 (about 17 min) and the other was being
  drained, so the probe, and with it the whole run, faced about 30 minutes of
  waiting before its own validation began.
- `eu-stack` of `serve`, main thread: `Runner::run` -> `Runner::drive` ->
  `std::thread::scoped::scope<Runner::pipeline ...>` -> the pipeline closure ->
  `Runner::handle_red` -> `std::thread::scoped::scope<Runner::validate_all ...>`
  -> joining a `Vec<ScopedJoinHandle<...>>` -> futex wait. Another thread:
  the `validate_all` closure -> `Runner::validate_one` ->
  `Runner::donor_validate` -> `thread::sleep` (polling the queued donor job).

The operator let the drained donor claim exactly the probe job (cap 1 until it
held that job, then cap 0) and turned `confirm_reruns` off from the next run.

Related: #1005. Both turn one slow or busy donor into a stall of the whole run.

# Design

Run the probe as an ordinary window batch, not synchronously inside
`handle_red`:

- `handle_red` marks the red batch as awaiting confirmation and starts the
  probe as a flight in the window. The main loop goes on iterating: it takes
  intake, handles other results, decides batches, and starts new ones.
- The red batch keeps its place for in-order decisions, as a batch cut out of
  the stack does. It is decided when the probe returns: a confirmed red ejects
  as today, and a green probe records the red as flaky as today.
- Optionally, donors claim probe jobs before batch jobs. A pending probe holds
  back the decision of every batch that jumped over the red one, so it is worth
  more than a new batch.

# Acceptance

- A test whose probe donor job stays queued. Before the probe returns, the run
  applies a recorded submit at a free slot, starts that batch, and decides a
  green rooted batch that did not jump over the red one.
- The probe's verdict decides the red batch exactly as today: eject on a
  confirmed red, flaky on a green rerun.
- With two reds in one run, the second red's probe starts without waiting for
  the first one's verdict.
