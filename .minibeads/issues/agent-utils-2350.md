---
title: 'vibe-talk-back-toast-covers-returned-row: the "Back where you were." toast covers the row Back returned to'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T22:37:00-05:00
updated_at: 2026-11-27T06:20:00-05:00
closed_at: 2026-11-27T06:20:00-05:00
---

# Description

Found by the advisory design review of #2270 vibe-talk-linked-bubble-follows-back (the #2261 summary-link walk's after-Back frames, phone 412 and desk 1280, light and dark): after Back from a citation jump the page says "Back where you were." on its floating status toast, and the toast is drawn over the very row Back returned to.

- Phone: the toast covers the returned row's body text (R5 in vibe-talk/docs/ui-rules.md: status must not compete with content).
- Desk with Discuss open: the toast runs under the Discuss panel, so its text is cut ("...messag" before Back) and its dismiss x is hidden after Back (R6).

It predates #2270 (the words are #2261's and #2273's); #2270 does not change the toast.

# Design

Not decided; to be agreed with the liaison before building. Options to weigh: say the return with no toast at all (the row and its Linked bubble standing where the reader left them is the confirmation), or place the toast where it holds no message text and is not under the Discuss panel, with nothing in the list moving when it appears or goes (rule 370, R12).

# Acceptance Criteria

- After Back from a summary-citation jump, on phone and desk, light and dark, with Discuss open: no message text is under the toast (or there is no toast), and nothing of it runs under the Discuss panel.
- The per-frame Back trace of the #2261 walk still holds the returned row within 0.5 px.
- Screenshots of the after-Back frame, before and after, each folder with its BUILD.txt.

# Notes

[claude-opus-5-5, lane cl-pagenits] Filed at the coordinator's request from the #2270 review (2026-11-26). The liaison put it in the page-nits list after #2349, the same class as #2314 vibe-talk-refreshing-pill-covers-message-text (floating chrome over text): fix it consistently with whatever the owner decides there (an R18 rule, if approved).

## Resolution

[claude-opus-5-5, lane cl-pagenits] Landed in a614cf64, 013e128e and a43456f5 (2026-11-27 6:16 AM ET, liaison LAND NOW; owner gate on 7e5f4c7a fully green, 238/238 steps; landed over item 9's screenshots.py change with --allow-overlap on a verified tree: 9045f773 checked whole (release build, fmt, scripts-check, tests/js 3493/3493, summary_link, firstrun, floating_chip_overlap, server_hits, discuss_browser, scenes 74 and 74-status-detail), then 2556c213, which differs from it only in issue files). The liaison's option A, in the coordinator's two commits: (1) the idle status line is clipped to nothing as the page's .sr-only is (1px, min-width/min-height 0, overflow hidden, clip-path inset(50%)) instead of `visibility: hidden`, which took it out of the accessibility tree, so idle words (Ready., the device-voice notices) are now really announced and still never drawn; its dismiss x is hidden with it, and the screenshot harness's `__visible` counts the clip as not visible; (2) "Back where you were." joins the idle words, so no toast stands over the returned row (phone) or runs under Discuss (desk). Tests: tests/js/page/shell.mjs (the idle rule), tests/js/view/shell-notices.test.mjs, tests/firstrun_browser.py (an idle word in the polite live region and the aria snapshot, drawn at most 1x1 px), tests/summary_link_browser.py (an observer records the status line the moment Back says its return: idle, announced, clipped; red against main's build), and the liaison's extra check: Back from a row with no Linked bubble puts the row back within 0.5 px with every row in view where it stood and no bubble left. The jump's own toast is #2365.
