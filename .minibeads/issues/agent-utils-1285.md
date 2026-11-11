---
title: 'front-starve-test-leak: the front-starve integration test leaks its donors on failure'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-11T11:00:00.000000000+00:00
updated_at: 2026-11-11T11:00:00.000000000+00:00
---

# Description

Leaks: two integration-test donors (d1) from the front-starve worktree ran on devbig014 since Nov 10 12:27 and 12:31 AM PST, claiming every ~1 s against /tmp/merge-queue-it-front-starve-*/queue. coordb reaped PIDs 2260308 and 3052251 at ~5:50 AM ET. The test doesn't reap its donors on failure or abort.

# Design

In `rs/merge-queue/tests/cli_front_starve.rs`: the test must reap its donors on failure and abort (a drop guard or a kill-on-exit process group).

# Acceptance Criteria

- The front-starve test's donors are killed on every exit path (pass, failure, panic, abort).
- `cargo test -p merge-queue --test cli_front_starve` passes.

# Notes

Reported 2026-11-11. Issue-only for now, unless trivial. Item 2 of the donor-jobs-cap findings relayed to this lane.
