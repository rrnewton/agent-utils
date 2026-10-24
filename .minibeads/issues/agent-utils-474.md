---
title: 'browser-lane-critical-path: the serialized browser lane is the whole gate''s wall time'
status: closed
priority: 2
issue_type: task
labels:
- validation
- vibe-talk
created_at: 2026-10-23T23:26:13.489587599+00:00
updated_at: 2026-10-24T01:33:18.406083525+00:00
closed_at: 2026-10-24T01:33:18.406083395+00:00
---

# Description

[opus 5.5] Found under #472 gate-flake-hardening. A full vibe-talk gate takes ~2430-2560 s (passing gates on 2026-10-23), and that is the sum of the browser steps' wall times (~2454 s in one profile), because tests.dag.yaml caps resource browser at 1 and every *-browser check and the screenshot walk hold it. Two steps are most of it: check.approval-card-browser ~700 s (was ~10 s before its scenarios accumulated) and check.column-overscroll-browser ~200 s; shots.screenshots ~490 s. Nothing else is on the critical path: after #472 the Rust and page suites (build.tests ~100 s, test.unit ~37 s, test.page ~510 s) run beside the lane. Options: let the cap be raised on a host with the cores for it (an override of resource_caps.browser), split approval-card-browser and column-overscroll-browser into shards by viewport, and/or look at why approval-card takes 700 s (fixed sleeps?). More browsers at once means more load per check, so it wants the #472 hardening first.
