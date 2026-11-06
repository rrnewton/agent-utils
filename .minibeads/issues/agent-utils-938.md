---
title: 'merge-queue-resubmit-at-new-head: a submit at a new head for an excluded change applies at the next intake point'
status: closed
priority: 1
issue_type: task
assignee: mu-mqdonor
created_at: 2026-11-06T13:00:23.885811025+00:00
updated_at: 2026-11-06T14:15:36.139751154+00:00
closed_at: 2026-11-06T14:15:36.139750914+00:00
claimed_at: 2026-11-06T13:00:26.606757613+00:00
claimed_until: 2026-11-08T13:00:26.606604082+00:00
---

# Description

Added by coordb (HIGH, do with #936). Submits at a new head != excluded head apply at next intake point and drop stale exclusion; same-head submit waits or is refused with reason; status/list name why a pending request waits. Optional: resubmit of in-flight at new head or --after-only may cancel in-flight and replan; pending withdrawal must not block unrelated greens.
