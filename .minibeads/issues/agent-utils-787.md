---
title: 'local-merge-queue: consumer-scale follow-ups (daemon stop mid-run, real-queue timing, generator timing)'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-02T18:52:14+00:00
updated_at: 2026-11-02T19:12:34+00:00
---

# Description

[opus 5.5] What stays open after #735 local-merge-queue closed (first-contact fixes and the reconciliation with a second lane landed in d6bdb8e8, c3c9aa22 and 802fad3f). Same slug; one issue for the remaining consumer-scale gaps the handed-over lane listed, plus one found while reviewing its last landing.

1. **Daemon stop during a run (suspected, unverified; first priority).** `serve` reads its SIGTERM/SIGINT flag only between cycles. Validators run in their own process group, so a terminal Ctrl-C leaves them running and the cycle finishes, possibly 40 minutes later. Under a service manager that signals the whole cgroup, though, they die with the daemon. A signal-killed validator returns no exit code and is not marked cancelled, so the run may read it as red, start a culprit search, and record ejections before the manager's stop timeout ends the process. Wanted: a regression test that sends the daemon SIGTERM mid-validation, then a fix. On shutdown, the run should cancel its validations (cancelled, never red), write no verdicts, and leave items queued.
2. **Time to first validation on a real queue.** Measured so far: the fixture's first batch (0.172 s) and a cold full plan on the consumer's ~85-change queue before the probe rewrite. Wanted: a plan on that queue at the current probe (the fixture went from 11 s / 265 CPU-s to 2.2 s / 6 CPU-s), and an end-to-end time to the first validation start.
3. **Per-generator timing.** Per-change regeneration costs about 32 s per change on the consumer's repository, and that figure is not split by generator. Wanted: per-command timing in the regeneration record. Skipping a regeneration when nothing changed was dropped by decision, so the speed-up has to come from the generators.
4. **Staged planning continuation is in memory only.** After a daemon restart mid-continuation, the planner re-probes everything; the persistent probe cache keeps that cheap. Low priority; listed so the gap is on record.

# Notes

[opus 5.5] (a) delivered in 88965b65. A stop is now never a verdict. SIGTERM or SIGINT cancels the validations, records them as cancelled, starts no batch, landing or ejection, and leaves undecided changes queued. A runner or generator killed by an outside signal stops the run with that reason instead of reading as red or `regen-failed`. The shipped unit sets `KillMode=mixed`. Five tests cover it, all failing on the previous code. Open: (b) and (c) need numbers from the consumer's real queue; (d) is low priority.
