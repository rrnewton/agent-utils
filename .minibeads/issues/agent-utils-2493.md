---
title: 'private-tmp-bound-test-report-race: the host-temp bound test child writes its report after the oversized file, so the bound can stop it before the report exists'
status: open
priority: 2
issue_type: bug
assignee: cl-flaketriage
labels:
- repository
- tests
- hot-host
created_at: 2026-11-28T14:08:00-05:00
updated_at: 2026-11-28T14:08:00-05:00
---

# Description

`py/tests/test_private_tmp.py::test_a_host_temp_root_past_its_bound_stops_the_command_with_a_named_refusal` runs `with-private-tmp --host-temp` with a 1,000,000-byte bound, polled every 0.05 s. Its child does two things in this order:
1. writes a 4,000,000-byte file into its temp root;
2. writes the root's path into the test's `report` file, then sleeps.

The wrapper stops the child as soon as a poll sees the root past its bound. If that poll falls between steps 1 and 2, the child dies before step 2. The test then fails reading `report` (FileNotFoundError), although the wrapper did exactly what the test checks: it refused, with exit 73 and the named message.

Seen three times on 2026-11-28 (`python.repository-infrastructure.test`):
- gate 1795879520528601755-56c13044 and gate 1795890126740230136-2a0515e3, in their kept step logs;
- one more gate, in the console log of the lane that ran it.

On a busy host the window between the two writes widens.

# Design

- Test-only: the child writes `report` FIRST, then the oversized file, so the report always exists once the bound can trip.
- The assertions stay: exit 73, the named refusal, the root gone, and the host base empty.
- No product change; no wall-clock budget (owner rule 346).

# Acceptance Criteria

- The test passes on a busy host; the child's report exists whenever the bound stops it.
- `python.repository-infrastructure.test` stays green in a gate.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the triage-set gate on c949532e.

Related:
- #2383: the private temp root and its host-temp bound.
