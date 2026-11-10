---
title: 'ghpaced-transient: a GH-PACED refusal must not host-fault a donor'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-10T06:15:00.000000000+00:00
updated_at: 2026-11-10T06:15:00.000000000+00:00
---

# Description

A donor validation that reports could-not-run (exit 75) because a git call inside it was refused by the host credential pacing guard (one login per interval per host; waiters past the wait bound exit 75 with a pacing-refusal line on stderr) is treated as a host defect. The donor's follow-up probe loses the same race, so the queue records a host fault for the donor and pauses it: at 2026-11-10T05:56:08Z the only local donor was faulted this way and its queue stalled. Shared-budget contention is not a host defect, and a host probe cannot fix it.

# Design

In `rs/merge-queue`:

- Classify a could-not-run whose cause is a pacing-guard refusal (the refusal line on the validator's stderr) as transient, beside the existing busy and stale-base classifications in `validate.rs`.
- Donor side (`Worker::validate` in `donor.rs`): a pacing refusal is never probed and never records a host fault; the receipt still re-queues the job. Count consecutive pacing refusals, and past a bounded count record the fault after all, so a lasting outage still shows up and eventually faults.
- Queue side (receipt handling in `donor.rs`): the re-queued job keeps its counted could-not-run strike, so the existing per-job bound still applies; the history names the pacing cause.

# Acceptance Criteria

- A fake pacing-guard refusal leads to a transient re-queue with no host fault recorded and the donor able to take work again.
- Repeated pacing refusals beyond the bound still record a host fault.
- `cargo test -p merge-queue` (all targets) passes, including the new cases.

# Notes

Fault decision found at `Worker::validate` (`rs/merge-queue/src/donor.rs`, the probe-and-fault block after a refused receipt) calling the `donor host-fault` route into `Store::record_host_fault` / `note_host_fault`, which writes `host-faults.json`. Evidence: a consumer queue's `host-faults.json` fault at 2026-11-10T05:56:08Z after pacing-refused validations.
