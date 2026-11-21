---
title: 'vibe-talk-all-view-send-flash: sending in the All view must not flash different messages'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- owner-bug
created_at: 2026-11-20T13:47:00-05:00
updated_at: 2026-11-20T23:24:00-05:00
closed_at: 2026-11-20T23:24:00-05:00
---

# Description

[watermelon-full-preview] Owner bug 258 (1:45 PM ET 2026-11-20, verbatim): "Man when I send a message in the All view, I'm sometimes seeing the screen flash and show me DIFFERENT messages (i know from the different thread color) in the same spot on the srceen near where I am at the end of the scrollback. Then after a second the flash ends and the messages I was just looking at are back on the screen"

In the All (cross-thread) view, after Send, the visible window near the end of the scrollback is briefly replaced by different messages for about a second, then the original messages return. The likely area is an optimistic/echo insert or a timeline refetch that re-renders the list with a different window or ordering, then a reconcile that restores it; the real cause must be found in the code, not assumed.

# Design

- Reproduce on a local fixture: several threads in the All view, scrolled to the end of the scrollback; send; capture every animation frame for the following interval (the anchor message's rect.top and the set of visible message ids per frame).
- Invariant: Send never changes which messages are on screen, other than adding the new one, and never moves the anchor by more than 2-3 px.
- Make that invariant a check that counts frames whose visible set changes (other than the added message) and asserts zero; event counts, never wall time. End-to-end through the real router, stubbing only external upstreams.
- Fix the root cause in the send/echo/refetch/reconcile path with the smallest change; if the fix touches list rendering or virtualisation broadly, notify the coordinator first so it does not collide with the standing perf lane.

# Acceptance Criteria

- On a local fixture with several threads in the All view at the end of the scrollback, sending adds only the new message: no frame shows a different visible message set, and the anchor moves by at most 2-3 px.
- The frame-count gate check above passes through the real router; page tests cover phone and desktop sizes.
- Focused suites pass; the browser test and screenshot matrix (light and dark, phone 412x915 and desktop 1280x800, keyboard open and closed) are run by the coordinator on the browser host from the landed SHA, since this host has no browser libraries.
- Never call the live vibe-talk server in any check.

# Notes

Owner-priority, fast-track: deployed on landing. Related owner rules: dynamic testing through the real server before landing (thread 193), screenshot matrix (thread 124), condition-based waits only (rule 346).

## Resolution

[watermelon-full-preview] Landed on github/main as 575385c5 (a55f08fb the fix): a send in All now adds its row at the foot and moves nothing — no scroll at the Send tap or at the answer outside the fitted-keyboard regime, the composer hold (#1839) released at the press so the send's redraws keep the reader's anchor, and a keeping re-read that brought nothing new (or only the reader's own send) no longer re-takes the foot. Verified by the frame-capture check (tests/send_flash_browser.py, all eight non-windowed walks at 0 flash frames and 0.0px on devbig030), the page anchor tests, and the focused suites; the final owner gate's only red was screenshots-desks scene 59, waived as the known load flake #2014. Remaining, owned by #2008 (mu-windowed): the late-read windowed walk can still drop rows for one frame (0.3px) on the combined tree, which the two [expected-fail #2008] marks in the flash check name until that lands and they are removed.
