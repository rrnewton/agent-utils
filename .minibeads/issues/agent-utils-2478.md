---
title: 'keyboard-settings-return-offset: keyboard_browser reads the list offset once right after Escape leaves Settings and fails "back from Settings the list was at 4059, not where the reader left it (4171)" on heads that never touched vibe-talk'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- tests
- hot-host
created_at: 2026-11-28T12:20:00-05:00
updated_at: 2026-11-28T12:20:00-05:00
---

# Description

`vibe-talk.check.keyboard-browser` (`vibe-talk/tests/keyboard_browser.py`) was red on 8 gate heads in the 7 days to 2026-11-28, 2 of them in the last 48 hours. 6 of the 8 heads never touched vibe-talk. The step output kept from 2026-11-28 at 3:06 AM ET, on a head with no vibe-talk change, is:

    AssertionError: back from Settings the list was at 4059, not where the reader left it (4171)

#2432 saw the same check fail once (4005 against 4100) while fixing another keyboard race. It noted the failure as a separate check, to be numbered if it recurred. It has recurred.

The walk presses Escape until `screen-main` is visible (`key_until`). It then compares the list's offset once against the offset recorded before Settings opened, with a 2 px tolerance. Showing the main screen and putting the list back at its place are not the same moment. On a busy host the offset read can land before the restoration (or the re-layout after it) settles. Whether the page ends at the right place is never asked.

# Design

- Wait for the restored offset as a condition: `|top - before.top| <= 2`, polled with the hang backstop (#2077) as the walk's other waits are.
- Record the offsets seen during the wait, so a failure tells a late restoration from a wrong one. A wrong one is a product bug in the place-keeping, and gets its own issue.
- If the restoration is late on an idle host too, report the time to the place as a page measurement (a span in the #2276 debug mode), not as a gate threshold.

# Acceptance Criteria

- The Settings-return check waits on the restored offset as a condition, and its failure message names the final offset and the offsets seen.
- The walk passes 20 of 20 runs with host load1 at or above 237 (75% of 316 hardware threads), recorded with the load per run.

# Notes

Filed by `cl-redaudit` (number pre-assigned by the coordinator, 2026-11-28), from the frequent-red gate audit of 2026-11-28.

- Only 1 of the 8 reds has its step output left. The others are counted, not proven, as the same case.

Related:
- #2432: first sighting, as a side note.
- #2077: hot-host waits.
