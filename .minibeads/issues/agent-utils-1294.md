---
title: 'donor-probe-pacing-bound: a pacing warning must not fault a donor host, and a restored pause must keep the configured probe bound with killed probes reaped'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-11T13:00:00.000000000+00:00
updated_at: 2026-11-11T13:00:00.000000000+00:00
---

# Description

Two defects in `rs/merge-queue` (donor probe path), found while recovering a donor host after a toolchain deletion made its host-scope probe fail for about 22 h. Related: #1271 probe-timeout-bound, #1273 probe-no-counted-tests, #1286 donor-probe-output, #1258 stale-donor.

(1) `donor.rs` counts a `GH-PACED WARNING` line as a pacing failure. That line only means a credential lookup waited about 9 s and then SUCCEEDED, so it must not fault the host. Today the classifier treats the warning alone as a pacing refusal, so a warning-then-success transcript can be counted as a pacing failure and fault a healthy host.

(2) The probe bound was taken from a paused snapshot written before #1271 (12151d2e) added `probe_timeout_secs`, so the snapshot lacks that key and the 60 s code default applied instead of the configured 180 s. Probes killed at 60 s left 3 zombie processes under the donor: the killed probe child was never reaped.

# Design

In `rs/merge-queue`:

- The pacing classifier counts only real pacing failures: a non-zero exit (a refusal receipt carrying `COULD_NOT_RUN`), or an explicit failure line (`GH-PACED REFUSED`). A `GH-PACED WARNING` on its own, in a transcript that then succeeds, is not a pacing failure and never faults the host.
- A restored or paused validator snapshot keeps the configured probe bound; a snapshot without the `probe_timeout_secs` key falls back to the live config value rather than the 60 s code default.
- A probe killed at its bound is reaped: the child and its process group are waited on, leaving no zombie children.

# Acceptance Criteria

- A warning-then-success transcript (a `GH-PACED WARNING` line, then success) is not counted as a pacing failure and faults no host; a test covers it.
- A snapshot without the `probe_timeout_secs` key uses the live config value for the probe bound; a test covers it.
- After a timeout kill, no zombie children remain (counted via `/proc` or `waitpid`, never timing); a test covers it.
- `cargo test --manifest-path rs/Cargo.toml -p merge-queue` (all targets) passes, including the new cases.

# Notes

P0 under the same-hour rule for merge-queue bugs. Reported 2026-11-11.
