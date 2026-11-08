---
title: 'merge-queue-donor-job-refusal: a job-caused could-not-run is not a donor fault, and the queue keeps its cause'
status: open
priority: 1
issue_type: bug
assignee: mu-donorref
created_at: 2026-11-08T13:15:00.000000000+00:00
updated_at: 2026-11-08T13:15:00.000000000+00:00
---

# Description

A consumer's queue (merge-queue b747d306, keep_stale_roots=true) on 2026-11-08: a remote donor leased a v7 batch at 12:50:02Z and reported admitted at 12:50:05Z. The base moved at 12:51:03Z. The donor's validator then refused with its stale-base class ("target lacks the fresh origin/main"), exit 75. Three things went wrong on our side:

- The queue recorded the teardown's LAST output line ("validate-run: RETIRE-RETAINED ... may still use slot ..."), which hid the stale-base cause.
- The job went back to queued and sweep_stale cancelled it, but the donor then SELF-PAUSED to probe, as if the refusal were a host fault. It held the job 5 min 15 s, 4 min 14 s of it after the base moved.
- The donor had reported "admitted" before the validator's own admission and pre-run checks had passed.

The consumer is changing its validator so that in queue mode it checks "target contains the job's base" rather than fresh origin/main. This issue is the merge-queue side.

# Design

1. Donor: a could-not-run exit whose cause is the JOB (the validator's stale-base refusal class) is reported as that class, records no host strike, and does not self-pause, matching sweep_stale's rule that a stale job is not a donor failure. Recognise it by a stable marker (an exit code plus a fixed refusal token, or a structured line) rather than free text where one exists; document which.
2. The queue and donor record the refusal's CAUSE line (the first refusal line, or a structured cause) instead of the teardown's last output line.
3. Backward-compatible follow-up, built like the tool-identity lines of #1045/#1049: an optional validator line meaning "my own admission and pre-run checks passed"; the donor reports admitted only after seeing it, and keeps today's behaviour when the line is absent (an optional strict config key may require it). Either a second commit here or a filed follow-up issue; the lane says which.
- Old validators and donors keep working.

# Acceptance Criteria

- A test: a donor job whose validator exits with the stale-base refusal is reported with that class, adds no host strike, and the donor does not self-pause; the next job is taken normally.
- A test: the recorded failure detail is the refusal's cause line, not the teardown's last line, when the teardown prints after the refusal.
- A test (or the follow-up issue's): with the admission line present, "admitted" is reported only after it; without it, behaviour is unchanged.
- A clean whole-crate merge-queue run (rust.merge-queue.test within its 2400 CPU-s budget).

# Notes

Reported 2026-11-08 by a consumer's queue operator, relayed by the lead coordinator; evidence (donor job record and run events) kept on the lead's host. Related: #1045, #1049, #1051.
