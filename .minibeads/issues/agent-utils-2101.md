---
title: 'vibe-talk-offline-cache-phone-check-unreliable: offline-cache-phone-360-browser fails in most gates across unrelated trees'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- testing
created_at: 2026-11-22T07:40:00-05:00
updated_at: 2026-11-22T07:40:00-05:00
---

# Description

vibe-talk.check.offline-cache-phone-360-browser failed in about 20 of the last 40 gate and rerun rows on one host, on trees that do not touch the offline cache, including solo exclusive reruns where it was the only selected failure. Either main is broken for this check or the check is not robust on a loaded host.

# Design

Reproduce on an untouched main tree (solo, several runs, with the host loaded as usual) and read the failing assertion. If main is broken, fix the cause. If the check depends on wall-clock timing or frame races, make it condition-based; if that cannot be done quickly, disable it in the gate (with this issue as the tracker) under the testing rule that nothing timing-sensitive gates, and re-enable when it is robust.

# Acceptance Criteria

- The cause is named with evidence; the check passes repeatedly on a loaded host, or is disabled from gating with a tracked path back.

# Notes

Filed by the coordinator from the gate ledger.
