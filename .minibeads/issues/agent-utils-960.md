---
title: 'merge-queue-root-near-landing: a new root planned while another root is green but unlanded should not go stale'
status: in_progress
priority: 1
issue_type: task
assignee: mu-mqdonor
created_at: 2026-11-06T19:35:46.404619718+00:00
updated_at: 2026-11-06T19:35:49.652156455+00:00
claimed_at: 2026-11-06T19:35:49.652156455+00:00
claimed_until: 2026-11-08T19:35:49.652059179+00:00
---

# Description

HIGH, from coordb. Option (b) chosen: defer the new root's base choice until the pending landing completes, bounded 60s, then fall back to today's behaviour; event recorded. Tests: submit during a pending landing gets the post-landing base; no deadlock if the landing fails; unchanged when no green is pending.
