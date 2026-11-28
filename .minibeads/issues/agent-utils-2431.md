---
title: 'merge-queue-registry-stops-at-home: registry discovery stops before the home directory, as agentctl does since #1169, so a queue never addresses the agentctl plugin home'
status: open
priority: 2
issue_type: bug
assignee: cl-tmpfix
labels:
- merge-queue
- agentctl
- tests
created_at: 2026-11-27T19:46:43-05:00
updated_at: 2026-11-27T19:46:43-05:00
---

# Description

merge-queue finds the agentctl registry for its notices with `registry_above(anchor)`: the nearest `.agentctl` at or above the queue's repository or state directory (#1472). It is used at `init`, for the working directory, and at delivery.

The walk goes to the filesystem root. So a queue under the home directory with no project registry reaches `~/.agentctl` and passes it to agentctl as `--registry`.

- `~/.agentctl` is agentctl's plugin home (the `AGENTCTL_HOME` default), never a project registry.
- agentctl stopped its own discovery at the home directory in #1169, so that a command under home never silently joins it. merge-queue never got that rule.

Found by #2383's second gate (27570ce1).

- With the run's temp root under the home directory, merge-queue's test fixtures walked up to a real `~/.agentctl`.
- `registry_above_finds_the_nearest_registry` got `Some(<home>/.agentctl)` where it expected None.
- Six CLI tests in `cli_notify`, `cli_precheck` and `cli_sync` saw an unexpected `--registry <home>/.agentctl` argv. Their fake agentctl refused it, so nothing was sent.

# Design

- The walk stops before the home directory (`HOME`), comparing canonical paths so a trailing slash or a symlink still stops it. This mirrors agentctl's `find_registry_from`.
- With no home, only the anchor itself is checked, never a walk to the filesystem root.
- `registry_above_until(anchor, home)` takes the home explicitly, for tests.
- A project registry below home is found as before. A queue outside home walks as before.

# Acceptance Criteria

- A unit test with a fake home shows that:
  - a registry below home is found;
  - neither `<home>/.agentctl` nor one above home is found;
  - a trailing slash in the home still stops the walk;
  - with no home, only the anchor is checked.
- `rust.merge-queue.test` passes with the run's temp root under the home directory (#2383) and under the host temp dir alike.

# Notes

Filed 2026-11-27 at the lead coordinator's request (number pre-assigned). The commit is stacked on #2383's tree, so one gate covers both.
