---
title: 'donor-cap-file: change a merge-queue donor''s job cap without a restart'
status: closed
priority: 1
issue_type: feature
assignee: cl-sync
created_at: 2026-11-06T01:32:43.607675868+00:00
updated_at: 2026-11-06T03:25:37.756546340+00:00
closed_at: 2026-11-06T03:25:37.756546170+00:00
---

# Description

A merge-queue donor's job cap can't change without a restart. `donor run --jobs N` is fixed for the life of the process, and SIGTERM gives back ALL in-flight validations (`Unreported::Released("donor stopping")`, donor.rs around line 2410). With a busy queue the donor never idles, so a restart gated on idle never happens. A consumer team currently runs a PATH shim that rewrites `donor claim --jobs` from a control file, as a stopgap.

# Design

Option 1 (preferred): `donor run --cap-file PATH`.
- Before each claim, read PATH. An integer N >= 0 makes the effective cap min(--jobs, N). 0 means drain: claim nothing, and do NOT trigger the until_idle exit.
- If PATH is absent or malformed, use --jobs, and log a malformed value.
- Running jobs are never touched.
- Log each change once: "cap now N (was M) from PATH".
- Show the effective cap in `donor status` and in the daemon log.

Acceptable alternatives: a queue-side `donor cap --donor NAME N|clear`, persisted in state and applied in Store::claim; or SIGUSR1 to drain and SIGUSR2 to resume. Not requested: any change to SIGTERM.

# Acceptance Criteria

- With 2 jobs running and a non-empty queue, cap 1 claims nothing until one reports, then holds at 1. No running job is released, and receipts are byte-identical to an uncapped run.
- Cap 0 drains without exiting. Clearing the file restores --jobs on the next poll.
- Tests: lowering the cap mid-run, cap 0 with jobs in flight, and a malformed value.

# Notes

Requested by the lead on 2026-11-05 at 9:18 PM ET (P1-ish), approved by a consumer team's lead. It pairs with #915 (the same donor code in rs/merge-queue), so it goes to the same lane: cl-sync, after #915. The lead's merge-queue lane owns that crate; keep its configuration working.

## Resolution

[opus 5.5 cl-sync] Option 1, `donor run --cap-file PATH`, re-read before every claim:
- A whole number N caps `--jobs` at N.
- 0 drains: nothing is claimed, and `--until-idle` does not exit.
- An absent or malformed file means `--jobs`. A malformed value is logged once.
- Running jobs are never touched, and SIGTERM is unchanged.
- Each change is logged once: "cap now N (was M) from PATH". The donor's "took" line names its cap.

`donor status` (text) shows each donor's cap and when it last claimed, from one fixed map file, `donor-caps.json`, that the queue writes at each claim. It is rewritten only when a cap changes or its last note is a minute old. A draining donor says so with the new `donor claim --drain`, which leases nothing. An older queue refuses that flag, which leases nothing either. `donor status --json` is unchanged.

Tests:
- Unit: the cap-file reader and the caps map.
- Integration, lowering the cap: from 2 to 1 with two jobs running, never more than one at once after the first report, no job given back, and the status shows cap 1.
- Integration, draining: cap 0 with a job in flight; it reports, nothing more is claimed, and the donor stays. Clearing the file resumes. A malformed value is ignored with a log line.
