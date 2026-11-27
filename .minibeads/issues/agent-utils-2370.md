---
title: 'shots-read-aloud-desk-click: the screenshot harness cannot click Read in the channel view on laptop-1280 (60 s actionability timeout, Discuss open or not)'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- screenshots
created_at: 2026-11-27T05:57:00-05:00
updated_at: 2026-11-27T05:57:00-05:00
---

# Description

Found while capturing the evidence matrix for #2351 vibe-talk-read-strip-foot-reader-moves. A temporary harness scene (never committed) loaded the page with a token, opened the channel pane, waited for its rows, stubbed an endless device voice (the harness's existing stub), and pressed Read with `driver.click("read-aloud")`. The same act worked on iphone15 and iphone-se. On laptop-1280 it never got past the click, with Discuss open or not: the run failed with "the walk into it did not complete (TimeoutError) ... Timeout 60000ms exceeded".

Trace notes from prints added to the scene:

- the page loaded and the channel's rows stood;
- `#audio-source` was shown with `aria-checked="false"` (no switch needed);
- `window.__visible('read-aloud')` was true;
- `driver.click("read-aloud")` (a plain Playwright `page.click("#read-aloud")`) never returned, so it was Playwright's actionability wait (visible, stable, enabled, receives events) that ran out at its 60 s default. None of the scene's own waits (15-20 s each) fired.

So on the desk profile something either covers `#read-aloud` in the channel view or keeps it from settling. Every Discuss-open read-aloud cell is blocked until this is fixed. The read_aloud_blocks walk presses the same button on a desk without trouble, but its fake API serves no Discuss panel, so it cannot stand in for that cell.

# Design

- Reproduce with one harness scene on laptop-1280: the channel open, then a click on `#read-aloud` with a short timeout. Record Playwright's actionability log (which check fails, and which element would receive the click), then fix whatever covers or moves the button, or the scene's way of pressing it.
- No page change unless the cause is a real page defect (a control that covers Read for a desk reader too). In that case it is a page bug with its own evidence.

# Acceptance Criteria

- A harness scene on laptop-1280 presses Read in the channel view and captures a read playing, with Discuss closed and with it open, in both schemes.
- The cause is named in this issue (the covering element or the failing actionability check).

# Notes

Filed at the coordinator's request from #2351's evidence work (issue file only); not dug into further on purpose. Related: #2351, #2368.
