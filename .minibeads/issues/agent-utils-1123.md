---
title: 'create-lock-hold-classification-cost: an agent create held the run-population lock 127 s, classifying 76,008 events per pass'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T11:00:09.000000000+00:00
updated_at: 2026-11-09T11:00:09.000000000+00:00
---

# Description

On 2026-11-09 at about 10:45Z on another host, one wrkslots create --slot-type
agent held the project-root (run-population) lock for 127 s. Each classification
pass read the whole event log: 76,008 events, about 19 s per pass. A validate-run
waiting behind it then refused on a config-change race, and that host failed its
health check.

There is no existing issue for event-log compaction or a classification cache.
#613 is the full-text index, which is unrelated.

# Design

Measure first:
- Where do the 127 s go: event classification, image work (slot-image
  provisioning), or the lock itself (waits, re-reads)?
- How many classification passes does one create make, and which ones run under
  the lock?
- Does #1108's shape (do the expensive reads before taking the locks, keep only
  the re-checks under them) apply to create?

Then propose one or both of:
- event-log compaction: a snapshot plus a tail, with readers accepting both;
  follow the migration rule for layout changes;
- an incremental classification cache keyed by the event offset (and a log
  identity), so a pass reads only the new tail; invalidated safely on any
  rewrite.

Keep every safety decision on fresh evidence. A cache may only skip re-reading
events it has already classified, never re-judging liveness.

# Acceptance Criteria

- A measurement of the create lock hold split by phase, on a registry of tens of
  thousands of events (a fixture is fine), recorded in Notes.
- A design for compaction and/or the cache, approved by the lead before
  implementation.
- After the fix, create's lock hold no longer grows with the event-log length.

# Notes

Reported 2026-11-09 by the lead coordinator from a consumer's host. Filed by the
devbig030 coordinator. Related: #1108 remove-lock-hold-starves-validate.
