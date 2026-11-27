---
title: 'read-aloud-jump-check-race: read_aloud_blocks_browser reads the fake server once at the jump''s paint, and fails on a busy host when the request lands just after'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- tests
- read-aloud
created_at: 2026-11-27T06:25:00-05:00
updated_at: 2026-11-27T06:25:00-05:00
---

# Description

`vibe-talk/tests/read_aloud_blocks_browser.py` is a gating browser check. Its step 3 (a tap on another block of the message being read jumps there) failed with "the jump asked for []" twice in about 40 walk runs on 2026-11-27, each green on rerun:

- on main 1347494d (no other change), at load1 116;
- on the #2367 vibe-talk-read-row-moves-start-stop tree, at load1 180.

Cause: after the tap the step waits only for the painted "6 of 7", then reads the fake API's request list once. The page paints a jump before speech answers (#2058 vibe-talk-reader-arrows-instant), so on a busy host the page's request for the code block (block 5) can reach the fake server just after the paint. The list is still empty at that one read. The step judges the timing of two independent events, which breaks the hot-machine rule (owner, 2026-11-10: gating checks wait on events or conditions, never on how fast the host is).

# Design

- Wait for the request with the walk's state bound (`wait_for` on `api.reads_of`), then check that it asked for block 5. Test-only; the page is unchanged.

# Acceptance Criteria

- Negative control: with the fake server holding block 5's request back before recording it, the old single read fails "the jump asked for []" and the new wait passes.
- The walk 10 times in a row green with load1 above 150, load1 recorded per run.
- A gate on the fix.

# Notes

Filed at the coordinator's request (issue file only). The fix is a separate test-only commit. Related: #2058, #2351, #2367, #1250 vibe-talk-timing-robust.
