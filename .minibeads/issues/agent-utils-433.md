---
title: 'read-strip-low: sit the floating read-aloud strip directly on the bottom chips row, with no gap'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T16:11:20.343921339+00:00
updated_at: 2026-10-22T17:08:40.235494890+00:00
closed_at: 2026-10-22T17:08:40.235494759+00:00
---

# Description

[muse watermelon] Owner, trying #376 read-aloud-blocks and #380 read-aloud-timeline: the floating orange-red strip hovers above the line with Collapse all / Expand all with a gap between. Putting it as low as possible puts it in better reach of the thumb and leaves the most real estate visible above it. Cause in web/voice.css: #read-steer bottom is chips-foot + scroll-tools-clearance + 2.6rem, reserving room for the transient status pill even while it is hidden. Fix at the source: the strip hugs the measured top of #scroll-tools as its height changes (wrapping, safe-area, composer rise, bars hidden), the status pill stacks above the strip while it is up, and the list foot keeps room so the last message scrolls clear of the strip.

# Acceptance Criteria

At 393x852 and on a desk, while a multi-block read plays, the strip's bottom edge meets the top edge of the chips row (#scroll-tools) with no gap (under 2px), and still does after the row wraps or empties. The last message can be scrolled clear of the strip. The status pill does not overlap the strip. Page tests and a Chromium walk with screenshots.
