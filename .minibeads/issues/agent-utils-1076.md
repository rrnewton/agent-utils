---
title: 'site-weekly-item-stamps: stop printing invented times of day on the site''s week-level rollup pages'
status: open
priority: 2
issue_type: task
created_at: 2026-11-08T17:18:34.760237943+00:00
updated_at: 2026-11-08T17:18:34.760237943+00:00
---

# Description

[opus 5.5] Calendar rollup pages print each work item with a time of day, but a weekly, monthly or quarterly rollup model sees every lower-level input tagged with that period's start, so the items carry midnight of the day (or the week's start) rather than when the work happened. Pages written for a narrowed --agent-scope already stamp week-level items with the local date only (render._rollup_item_stamp, date_only_above_daily). The site's own pages (summary_data/, build and export) still print the invented times, because the Rust edition renders them byte-identically and test_wrkviz_build_rust requires the two to match.

# Acceptance Criteria

Site weekly, monthly and quarterly rollup pages stamp items with the local date only in both editions (rs/wrkviz/src/build_render.rs local_time and py/wrkviz/render.py rollup_markdown); daily pages keep phase times; the build/export differentials pass with the new format; one test pins each format.
