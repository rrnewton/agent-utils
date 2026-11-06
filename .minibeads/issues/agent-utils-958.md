---
title: 'merge-queue-stop-orphans-local-validation: stopping serve stops the local donor validation unit for cancelled jobs'
status: closed
priority: 2
issue_type: task
assignee: mu-mqdonor
created_at: 2026-11-06T12:54:29.567647039+00:00
updated_at: 2026-11-06T19:32:58.707818273+00:00
closed_at: 2026-11-06T19:32:58.707818143+00:00
claimed_at: 2026-11-06T13:54:49.178102883+00:00
claimed_until: 2026-11-08T13:54:49.177962923+00:00
---

# Description

Bundle B item 2 (MEDIUM). Stop local unit within ~30s on serve stop, orphan sweep at next serve start before admitting work, exit log names units left running.
