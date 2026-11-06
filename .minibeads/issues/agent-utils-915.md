---
title: 'merge-queue-keep-running-roots-only: keep only leased or running stale roots and re-plan queued ones without a strike'
status: closed
priority: 1
issue_type: bug
assignee: cl-sync
created_at: 2026-11-06T01:31:20.338452337+00:00
updated_at: 2026-11-06T02:24:17.877328025+00:00
closed_at: 2026-11-06T02:24:17.877327774+00:00
---

# Description

With `keep_stale_roots` on (and the donor-sweep change 69c2bca1), the merge queue keeps UNSTARTED stale roots as well as running ones. They are dispatched on the old base, and the consumer's validate-lock freshness gate refuses them. Each refusal holds a validation slot for 6.6-9.6 minutes, and the changes land about 45 minutes late. The cost is roughly 60 slot-minutes per landing with the flag on, against about 33 with it off, so the consumer turned the flag off again.

# Design

- Keep only LEASED or RUNNING stale rooted jobs, the work already paid for.
- CANCEL QUEUED (unstarted) stale rooted jobs, with NO strike against the change, and re-plan them onto the new base. Requeue them at the front so they don't lose their place.
- Code: rs/merge-queue: the donor sweep and prune from 69c2bca1, and the roots in run.rs from 05f35064.

# Acceptance Criteria

- The lead has a write-up with a test that fails today; it lands as a regression test here.
- Test: with keep_stale_roots on and a base advance, a queued stale root is cancelled with no strike and re-planned at the front on the new base; a running stale root is kept.
- Slot-minutes per landing with the flag on are no worse than with it off, in a scenario test or the consumer's next measured run.

# Notes

Requested by the lead on 2026-11-05 at 8:57 PM ET (P1). It can't start on the lead host until slots can be created there again, so it goes to an agent-utils lane here: cl-sync. The lead's merge-queue lane owns rs/merge-queue, so keep their configuration working and coordinate through the coordinator.

## Resolution

[opus 5.5 cl-sync] Under `keep_stale_roots`, only roots a donor has started (deciding job leased or finished) are kept when the base moves.
- A root whose job is still queued, or not yet published, is cancelled through the run's own path. It gets no strike, and its changes go to the front of pending to be rebuilt on the new base.
- The claim-time sweep exempts leased rooted jobs only.
- Local validation and compare-only offload behave as before. There is no interface or config change.

The write-up's scenario is a regression test in `rs/merge-queue/tests/cli_donor.rs`, and it fails on the previous code.

Unverified: slot-minutes per landing with the flag on, against off. A queued stale root now costs no donor slot at all, where each refusal of one used to hold a slot for 6-10 minutes. The consumer's next measured run with the flag back on will confirm it.
