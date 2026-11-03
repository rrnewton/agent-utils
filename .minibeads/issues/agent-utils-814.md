---
title: 'merge-queue-validator-exec-failure-not-red: a validator that cannot run is an infrastructure refusal, never a red verdict'
status: open
priority: 1
issue_type: bug
assignee: cl-mq@devbig030
labels:
- merge-queue
created_at: 2026-11-03T11:09:26.739150948+00:00
updated_at: 2026-11-03T11:09:26.739150948+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator (P1), from a consumer team's first live merge-queue serve run. The validator command could not be found (the validate adapter exited 127 because a tool it calls was not on the unit's PATH), and serve EJECTED the change as failed tests. A validator that cannot execute is an infrastructure failure, not evidence about the change; ejecting on it punishes the change for the host's setup.

# Design

- A validator that cannot execute (exit 126 or 127, a missing binary, a spawn failure, or a shell wrapper's command-not-found exit) is an INFRASTRUCTURE refusal: no verdict, the change stays queued, retry with backoff, and an alarm to the operator naming the command and the PATH it ran with. Never a red verdict that ejects.
- Donors: a donor that cannot run the validator declines the job (no receipt), with the same naming.
- Startup preflight (the #787 local-merge-queue preflight) resolves the validator command and fails loudly at serve startup when it cannot be found.
- Documentation recommends that the serve unit set an explicit PATH.

# Acceptance Criteria

- A test with a nonexistent validator binary: the change stays queued, no failed-tests verdict, an alarm names the command and PATH, and a retry follows with backoff.
- A test where a shell-wrapped validator reports command-not-found behaves the same.
- A donor that cannot run the validator declines the job.
- serve refuses to start (or warns, per its preflight mode) when the validator command does not resolve.
