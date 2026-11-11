---
title: 'proc-scan-bound: the remove census bound is a fixed 30 s on hosts of any size'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- census
created_at: 2026-11-11T16:00:00.000000000+00:00
updated_at: 2026-11-11T16:00:00.000000000+00:00
---

# Description

[watermelon-full-preview] On a host with 6,723 processes and validations running, four coordinator-authorized removals in a row refused rc 3: "process use is indeterminate because the /proc scan before the path fence exceeded its 30-second bound" (owner report via a coordinator, 2026-11-11). The same signature made five tests red on another lane's gate (the standalone lifecycle remove, the shared-store remove differential, the finish-journal differential, the cache-glob remove differential, and the process-path census test; each passed solo), and repeatedly reddened the #1287 retire-lock-narrow gates. While the bound trips, every removal on the host refuses, so slots cannot be reclaimed at exactly the load where reclamation matters most.

Cause: `_AGENT_REMOVE_BATCH_SCAN_SECONDS = 30.0` in `py/wrkslots/cli.py` is one fixed CPU-time bound for the in-process /proc census (one pass over /proc/*/{cwd,root,fd,maps} per slot check), whatever the host's process count. The scan's cost grows with the process population and with per-process fd/maps size; the neighbouring census budgets already scale with the host (the user-systemd stdout budget by process count, the validate-batch census at 120 s plus 60 s per extra sealed slot), but this one never did. There is no config key or environment override: an operator cannot raise it on a large host.

# Design

- The bound resolves as: `WRKSLOTS_PROC_SCAN_BOUND_SECONDS` environment override, then the `proc_scan_bound_seconds` configuration key, then a host-scaled default: a 30 s floor plus 10 s per 1,000 live processes (`_host_process_count`). The per-thousand allowance comes from the measured same-UID census costs recorded beside the validate-batch bounds: 1.1-1.3 s at about 4,000 processes idle, 11 s at load 42-68, and 32 to over 52 s at load 170-222 with 4,700-5,000 processes -- about 10 ms of CPU per process at the worst measured load. At 6,723 processes the default bound is about 97 s instead of 30 s.
- Fail-closed is unchanged: an exceeded bound still reports process use as indeterminate and still refuses; only the size of the bound changes.
- Tests that exercise remove and retire must not depend on wall time: the scaling rule gets a unit test (floor, scaling, config and environment precedence, counted from a stubbed process count, never timed), and census-driving tests may set a generous bound through the environment override instead of relying on the default.
- Rust parity: the Rust edition's remove census (`capture_process_snapshot`) has no fixed time bound to scale, so there is no bound to change there; it learns the new configuration key (validation and docs) so a config naming it is accepted by both editions.

# Acceptance Criteria

- On a many-thousand-process host under load, `wrkslots remove` and `retire-pending` no longer refuse solely because the /proc census crossed a fixed 30 s: the default bound scales with the live process count, with the 30 s floor kept.
- `WRKSLOTS_PROC_SCAN_BOUND_SECONDS` and the `proc_scan_bound_seconds` config key each set the bound explicitly (environment wins), and an invalid value refuses visibly rather than silently reverting.
- An exceeded bound still refuses as indeterminate (fail-closed), with the bound it exceeded named in the refusal.
- A unit test pins the resolution rule from a stubbed process count; no test's result depends on the wall time a census takes.
- The Python and Rust editions accept the same configuration.

# Notes

Reported 2026-11-11 (owner Slack 6228 via a coordinator; gate evidence from two other lanes the same day). Direction from the owner the same day: where recorded owner evidence (pidfd, cgroup membership, recorded start time) can decide liveness, prefer it over a host-wide walk; that fuller design is a separate follow-up, and this issue keeps the narrow bound fix. Blocks #1287 retire-lock-narrow landing, whose touched tests are census-bound-sensitive under gate load.
