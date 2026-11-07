---
title: 'merge-queue-donor-restamp-race: a remote donor whose tools are newer than a job''s stamp declines it, and the local donor takes the re-stamped job first'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T04:46:35.000000000+00:00
updated_at: 2026-11-07T04:50:00.000000000+00:00
---

# Description

With `validator.tool_identity` set, the queue stamps each donor job with the
queue host's tool identities when it publishes the job. A remote donor
refreshes its tools (`validator.tool_refresh`) before it validates, and it
declines the job when its identity still differs: "this donor's tools still
differ from the job's after refreshing them (...), so this donor is newer than
the job or unrelated to it". The queue then re-stamps the job with its current
tools and logs that the remote donor "may claim it again". But the queue
host's own donor shares the state and needs no refresh, so it claims the
re-stamped job first. While the tool repository's main moves faster than one
validation, remote donors decline every job, and everything runs on the local
donor.

Measured on a consuming project's queue (2026-11-07, 04:16-04:28Z). A remote
deciding donor leased 3 of 4 batch jobs and declined each within 3 to 4 s on a
tool-tree mismatch (job stamped 328b15c8, donor 635df031: the tool repository's
main had moved between the stamp and the donor's refresh). All 4 jobs then ran
on the local donor at once. The compare-only duplicates were declined the same
way by a second remote donor.

That queue's isolation tests fail when several validations share one host,
so the race turned into four false reds in one run. The operator capped the
local donor at 1 with `donor run --cap-file` as a stopgap.

The queue host's own stamp can be stale too. At every claim, `donor claim`
answers `validator.tool_identity` on the queue host (cli.rs, the
"Answer the tool identity now" step) and re-stamps queued jobs, but the queue
host never runs `validator.tool_refresh`; only donors do. So its answer is as
old as the last fetch of its tool checkout, which on that host happens when
its own donor refreshes at a claim. While that donor was busy with 3 jobs, the
answer stayed about 18 minutes behind the tool repository's main. Jobs were
stamped behind, and every refreshed remote donor saw itself as newer. The
operator now fetches the tool checkout every 60 s as a stopgap.

# Design

Keep the exact-tools rule; a job is still validated only with the tools it
names. Two changes, either sufficient:

1. Stamp at lease time: `donor claim` runs the queue host's `tool_refresh`
   (rate-limited) and re-stamps the job before it hands out the lease, so the stamp is as new as
   the donor's refresh, short of a race within seconds.
2. After a "donor is newer than the job" decline, re-stamp and reserve the job
   for the declining donor for one claim interval before any other donor may
   lease it.

# Acceptance

- A test with a remote donor whose refreshed tools are newer than the job's
  stamp: after its decline and the re-stamp, that donor gets the job on its
  next claim even while the local donor is idle. It fails on the current code.
- The USER_GUIDE donor section states which donor gets a re-stamped job.
