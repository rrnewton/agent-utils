---
title: 'done-feedback-grey-ring-window: done-feedback-browser reads a 900ms hold by wall clock'
status: open
priority: 2
issue_type: bug
labels:
- validation
- vibe-talk
created_at: 2026-10-24T01:33:11.516704504+00:00
updated_at: 2026-10-24T01:33:11.516704504+00:00
---

# Description

[opus 5.5] Found under #474 browser-lane-critical-path. done-feedback-browser's walk_motion presses e, reads the row, waits 450ms of wall time, and asserts data-done-flash is still 'marking'; the page holds it DONE_FLASH_HOLD_MS = 900 ms. If the press, the two evaluate round trips and the wait take more than 900ms on a starved host, the read sees 'fading' and the check fails ('the grey ring did not hold'). Seen once: the browser lane at cap 4 on a quiet host (/tmp/cl-browser/lane-n4a.log, read at 6s). Not reproduced in 42 further runs: dagrun --stress 12 at 4 at once (12/12), --stress 24 at 8 (24/24), the process-group freezer 50/100/15/400 (6/6), plus every lane and gate run since. A fix that keeps the check honest would measure the elapsed time on the page's clock (performance.now() at the keydown and at the read) and assert 'marking' while it is under 900ms, failing on 'fading' only when it is.
