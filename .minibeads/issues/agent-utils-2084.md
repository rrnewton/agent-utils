---
title: 'validation-cpu-cap-rule: size step CPU caps from measured history with a contention factor'
status: open
priority: 2
issue_type: bug
labels:
- testing
- validation
created_at: 2026-11-21T18:45:00-05:00
updated_at: 2026-11-21T18:45:00-05:00
---

# Description

Validation step CPU caps are hand-set from quiet-host runs. On a busy host, CPU seconds inflate by roughly 1.7x to 2.1x (SMT and cache contention), so steps time out with no test failure. In one loaded gate, 30 of 111 measured steps sat under three times their loaded CPU, several at about 1.1x.

# Design

A rule: cpu_timeout >= 3 x the step's p90 solo CPU from the profile history, with a 60 s floor for small steps; a report comparing caps with the rule in make check, flagging any cap under it when the DAG is edited; raise only non-census steps (census-driven steps shrink through the hermetic census work in #2065 first).

# Acceptance Criteria

- The cap report runs in make check and lists steps under the rule; caps raised per the rule for non-census steps; tests for the report.

# Notes

Filed by the coordinator from mu-hotcensus's measurements; the owning lane refines this file as it works.
