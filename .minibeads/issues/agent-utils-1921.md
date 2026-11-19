---
title: 'wrkslots-census-mount-namespace-blind-spot: census matches path text only, so a holder in another mount namespace is invisible'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- census
depends_on: {}
created_at: 2026-11-19T21:01:33+00:00
updated_at: 2026-11-19T21:01:33+00:00
---

# Description

[watermelon-full-preview] The all-process reference census matches a holder's /proc link and map targets by path text only. A process in another mount namespace that bind-mounts a subdirectory of a slot at a different path is therefore invisible to the census: its links and maps name the bind path, not the slot path. The B2 cold re-review reproduced this with real processes (an unprivileged user+mount namespace holder over a bind-mounted slot subdirectory): a validate teardown closed and deleted the tree under the live holder. The base shares this blind spot (its same-UID census also found no match for that holder when it closed); this is not a regression introduced by the B2 closures, which reuse the same census. Tracking it here so the gap is owned rather than rediscovered.

# Acceptance Criteria

A design note states whether device/inode matching (the same-UID census already has an inode stage) can cover cross-namespace bind mounts of slot subtrees, what it costs on a busy host, and which closure paths (general census, absent-slot recovery, validation teardown) would adopt it. If adopted, a real-process regression test holds a slot subdirectory through a bind mount in another mount namespace and the relevant closure refuses.

# Notes

Found by the second B2 cold review of the census closures stack. No behaviour change is requested until the design note is reviewed; the current censuses fail closed in every other probed case.
