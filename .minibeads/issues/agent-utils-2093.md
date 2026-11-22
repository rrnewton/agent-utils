---
title: 'wrkslots-plain-checkouts-image-backed: make plain-directory validation checkouts image-backed so busy-ness has a kernel answer'
status: open
priority: 2
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-22T06:05:00-05:00
updated_at: 2026-11-22T06:05:00-05:00
---

# Description

Plain-directory checkouts have no kernel busy count, so proving them unused needs a process scan; image-backed checkouts can be proven unused atomically by a non-lazy unmount.

# Design

Provision validation checkouts as image-backed (subvolume or loop image) by default, keeping a plain-directory path only where an image cannot be used, and document the remaining scan path for those.

# Acceptance Criteria

- New validation checkouts are image-backed by default; the busy check uses the unmount path; tests.

# Notes

Filed by the coordinator from owner directions relayed by a consumer's lead; the owning lane refines this file as it works.
