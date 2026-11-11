---
title: 'stale-donor: a donor that stopped polling must not defeat the claim-timeout suspension'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-10T23:55:00.000000000+00:00
updated_at: 2026-11-10T23:55:00.000000000+00:00
---

# Description

The claim timeout (3600 s) is suspended only while every eligible donor is host-faulted. In one run (20261110T020453Z era, report 2026-11-09 about 11:35 PM ET), one donor was paused by its own host-side disk guard and had not polled for about 6 hours (last seen 348 minutes earlier), yet still counted as eligible and healthy. When the only other verdict donor was host-faulted, the suspension did not apply and the queue cancelled five of six donor jobs with "no donor took it in time" (nothing ejected). A donor that is not polling at all is not healthy, so the exemption never fired.

# Design

In `rs/merge-queue/src/donor.rs` (`Store::every_eligible_donor_faulted` and the donor liveness records):

- A donor whose last claim poll (`seen_unix`) is older than a configurable bound (default 15 minutes) is not counted as eligible-healthy for the suspension check.
- A donor that reports a self-pause (for example its disk guard) registers as faulted/paused, so it no longer counts as eligible-healthy either.
- A stale donor that resumes polling counts as eligible-healthy again.

# Acceptance Criteria

- One live donor faulted plus one stale donor (no poll past the bound): the suspension holds and no jobs are cancelled.
- A stale donor that resumes polling counts again for the suspension check.
- `cargo test -p merge-queue` (all targets) passes, including the new cases.

# Notes

Report read at the 2026-11-09 stale-donor write-up relayed to this lane (queue-side decision in `donor_validate`, suspension via `every_eligible_donor_faulted`). No existing tracker entry covers this (searched: donor issues are unrelated).
