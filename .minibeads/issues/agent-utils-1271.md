---
title: 'probe-timeout-bound: the donor kills its health probe at a fixed 60 s, so a slow host probe never clears the fault'
status: closed
priority: 0
issue_type: bug
created_at: 2026-11-11T05:00:00.000000000+00:00
updated_at: 2026-11-11T05:00:00.000000000+00:00
---

# Description

A faulted donor is leased no work and probes its validator on a back-off (60 s doubling to 5 min) with MQ_PROBE=1; a pass clears the fault; a killed probe counts as failed.

What happened (2026-11-11): the local donor was faulted at 02:50:54Z (the validator's entry-lock wait expired after 900 s, held by a worktree-slot create that had exited by ~02:52Z). It probed 8 times from 02:50:54Z to 03:24:43Z, and all 8 were recorded as failed. A manual probe with the exact validator command and MQ_PROBE=1 PASSED (exit 0, "host-scope probe: ready") but took 59.2 s wall (classification 22.2 s with 2,046 run handles and 79,102 wrkslots events, plus ~20 s of tool start-up behind the GitHub credential pacer). rs/merge-queue/src/donor.rs probe_once kills the child at a fixed 60 s (started.elapsed() <= Duration::from_secs(60); line 3272 at 2b55c42d, 3321 at d9c98065), and the in-probe retries (PROBE_LOCK_WAIT_ATTEMPTS = 3) are each killed at the same 60 s. The fault was cleared by hand at 03:29:38Z; the donor did no work for 39 min.

# Design

In `rs/merge-queue`:

- Make the probe bound a validator setting, e.g. `validator.probe_timeout_secs`, default 60 (a consumer would set 180).
- A killed probe's recorded result names the timeout ("probe killed at the 60 s bound"), so it is distinguishable from "not ready".

# Acceptance Criteria

- A probe that needs longer than 60 s passes (and clears the fault) when `probe_timeout_secs` is set above its duration; the default stays 60.
- A probe killed at the bound records the timeout as its result, distinguishable from a ready/not-ready verdict.
- `cargo test -p merge-queue` (all targets) passes, including the new cases.

# Notes

Reported 2026-11-11. The consumer side sets its own probe budget to match (their change, not ours).
