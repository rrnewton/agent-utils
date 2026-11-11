---
title: 'paced-edges: align pacing classification and keep pacing out of the job strike limit'
status: closed
priority: 1
issue_type: task
created_at: 2026-11-10T18:15:00.000000000+00:00
updated_at: 2026-11-10T18:15:00.000000000+00:00
---

# Description

Follow-up to #1193 ghpaced-transient, from the adapter half landing with the matching behavior: the adapter counts a WARNING-only pacing failure as paced (it retries, then reports job scope), while the donor's receipt classifier counts only REFUSED, so the two sides disagree on the same output. Separately, a pacing refusal still counts toward the per-job could-not-run limit (3), so host-wide pacing contention alone can cancel a candidate's job.

# Design

In `rs/merge-queue`:

- `validate::gh_paced_refusal` matches a pacing WARNING as well as a REFUSED line: the adapter is authoritative about its refusal cause, and a WARNING-only exit-75 refusal is pacing-caused.
- Queue side (`Store` receipt handling in `donor.rs`): a pacing refusal records no could-not-run strike (like a stale-base refusal), so it cannot reach the per-job bound or decline the donor for that job. The job is queued again with the pacing note; the donor's consecutive pacing count, the claim timeout, and the item deferral bound still bound the loop.

# Acceptance Criteria

- A WARNING-only pacing refusal is classified paced on both sides (no host fault, no strike).
- Repeated pacing refusals alone never cancel a job at the could-not-run bound; the job stays queued.
- `cargo test -p merge-queue` (all targets) passes, including the new cases.

# Notes

Adapter behavior landed in the consumer's tree (census retries on REFUSED, PUSHBACK or WARNING, then a job-scoped pacing report). PUSHBACK naming stays adapter-side; the donor keys on the REFUSED/WARNING lines.
