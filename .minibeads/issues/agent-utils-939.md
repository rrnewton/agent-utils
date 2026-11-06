---
title: 'merge-queue-donor-refusal-not-host-fault: COULD_NOT_RUN refusal is not a host fault unless the donor probe also fails'
status: in_progress
priority: 1
issue_type: task
assignee: mu-mqdonor
created_at: 2026-11-06T12:54:29.564570813+00:00
updated_at: 2026-11-06T12:54:33.437014818+00:00
claimed_at: 2026-11-06T12:54:33.437014818+00:00
claimed_until: 2026-11-08T12:54:33.436898223+00:00
---

# Description

Bundle B item 1 (HIGH) from a consuming queue operator. Probe-gated fault, busy never faults, self re-probe 60s-5min without host-pause.json, donor clear-fault never leases, status/heartbeat fault line with last probe.
