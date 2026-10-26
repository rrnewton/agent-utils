---
title: 'wrkviz-archive-at-scale: archive-primary ingest, compressed upstream sources, and site responsiveness at months-long scale'
status: open
priority: 1
issue_type: feature
labels:
- wrkviz
created_at: 2026-10-26T15:03:24.361338515+00:00
updated_at: 2026-10-26T15:03:24.361338515+00:00
---

# Description

[opus 5.5] A months-long, multi-coordinator archive exposed three problems.

1. An operator compressed the upstream log mirror in place (X -> X.zst). Every importer opens X, so every team whose sources were compressed refused with 'missing' or 'disappeared', though no record had changed. Ingest should read a .zst spelling transparently.
2. Upstream transcripts are not retained forever (rotation, size caps, deletion). wrkviz's own archive is the primary record and upstream is a feed: a previously imported source that upstream no longer has must not refuse the run. Keep the archived copy, record the source as upstream-retired, and keep refusing only when upstream content changed in place for records already imported.
3. At this scale (12 coordinator teams over about 11 weeks), measure the built site's load time, memory, DOM size and frame times while scrolling and zooming in Chromium, and improve collapse, virtualisation and aggregation until it stays smooth.

Acceptance: tests for a rotated-away upstream file, a compressed upstream file, a record changed in place, and an incremental append after a checkpoint; the user guide documents the archive-primary model; before/after responsiveness numbers recorded.
