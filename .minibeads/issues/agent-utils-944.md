---
title: 'gate-flake-cross-environment-cpu-timeout: python.repository-infrastructure.cross-environment hits its CPU timeout under load'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T14:30:00.000000000+00:00
updated_at: 2026-11-06T14:30:00.000000000+00:00
---

# Description

python.repository-infrastructure.cross-environment failed in 4 first-run gates in about 24 h on its CPU timeout and passed when rerun alone.

# Design

Find which sub-check spends the CPU. Cut redundant work, split the step, or make its budget scale with measured load; keep the hermeticity checks intact.

# Acceptance Criteria

- 20 consecutive runs pass under bounded contention.
- Its assertions are unchanged.

# Notes

Filed 2026-11-06 9:30 AM ET at the lead coordinator's request, from one host's gate ledger and disk survey.
