---
title: 'vibe-talk-documents-fresh-load-bottom: a fresh Documents load jumps from the bottom to the middle'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
- ui
created_at: 2026-11-20T16:55:00-05:00
updated_at: 2026-11-20T23:22:00-05:00
closed_at: 2026-11-20T23:22:00-05:00
---

# Description

[watermelon-full-preview] Owner report 271 (2026-11-20): "we've got to fix it scrolling because it's supposed to start at the bottom when I load the documents page fresh and it's jumping to the middle senselessly." A fresh load of the Documents view must land at the bottom (newest), and stay there. It likely shares a cause with #1999 vibe-talk-documents-download-jump (whatever re-sorts or reflows the list).

# Design

- Trace the fresh load the same way as #1999: every frame for about 2 s, scrollTop against scrollHeight-clientHeight, naming which writer moves it.
- Fix the landing so a fresh load lands at the bottom and stays there while fetch states, toasts and meta lines settle.
- Add a browser test: after a fresh load the list is at the bottom within 1px once settled, and never visibly leaves it.
- Own commit, separate from #1999 if the causes differ. Do not touch the toolbar (another lane owns it).

# Acceptance Criteria

- After a fresh load the Documents list is at the bottom within 1px once settled, at phone and desktop sizes.
- The list never visibly leaves the bottom during the settle window in a per-frame trace.
- The moving writer is named from the trace.
- Existing documents suites stay green.

# Notes

Owner-priority, fast-track, same lane as #1999. Browser verification runs on the host with browser libraries; this lane hands the exact commands to the coordinator.


## Resolution

[watermelon-full-preview] Landed on github/main as eb52c80c (with #1999 as 7e341b3a), deployed by coordb 2026-11-20. A following refresh now lands through landAtFoot, which re-asserts the foot on the frames just after the landing while the scroller still stands where the landing left it, so heights settling after the landing cannot strand a fresh load mid-list; a reader who scrolls away keeps their place. Proof: per-frame fresh-load traces over 120 documents at 412x915 and 1280x800, on navigation and reload (settled gap 0.0px, never visibly leaves the bottom), locally and on the coordinator's host; documents JS suite and typecheck green; gate red only on five waived main/load failures.
