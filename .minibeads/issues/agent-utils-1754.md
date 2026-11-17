---
title: 'local-ci-hub-step1-host-limits: host-limits file, live admission and limp mode for local-ci-hub'
status: open
priority: 1
issue_type: task
labels:
- local-ci-hub
created_at: 2026-11-17T05:02:48.000000000+00:00
updated_at: 2026-11-17T05:02:48.000000000+00:00
---

# Description

Step 1 of the host-limits work: a Python parser for the host-limits
file (8 keys), live admission for validation runs, built-in limp mode
when no host file is installed, and the matching doctor reporting.
No host file is created by this work; a host admin installs one by
hand later. The gate reads none of the new keys in this step.

# Design

- One new module in the Python package owns the file: TOML loading,
  per-key effective values with documented fallbacks, headroom
  sampling, free-space measurement and read-only gate counting.
- Live admission replaces cap-sum arithmetic: young runs are charged
  until their use is visible, refusal waits on events to a
  caller-set bound and then reports no result, and gate runs are
  counted read-only without authority over gate state.
- A missing host file is limp mode, never a refusal: conservative
  built-ins, a warning on every run, and an error only in doctor.
- Host-imposed box breaches map to no result in this tool's own
  result handling only; sweep deletions defer while a measurement
  hold or host quiet is active on the existing defer surface.

# Acceptance Criteria

- All 8 keys parse with strict types; each malformed value falls
  back alone, with the disk floor falling back to the stricter of
  its two bounds, and doctor reports every fallback.
- Admission admits and refuses exactly on the live rule, including
  the young-run charge, the cleanup-pending bound, the disk floor
  and the unknown-gate case; refusal never creates a checkout or a
  failure row.
- Deterministic tests with injected snapshots and fake clocks cover
  both sides of each boundary; no test sleeps or asserts on wall
  time.
- Focused tests pass directly; the full local validate and the gate
  pass on the final tree before landing.

# Notes

Spec and background are held out of tree by the coordinator; this
issue records only the behaviour above, with synthetic fixtures.
