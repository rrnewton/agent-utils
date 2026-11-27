---
title: 'vibe-talk-back-toast-covers-returned-row: the "Back where you were." toast covers the row Back returned to'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T22:37:00-05:00
updated_at: 2026-11-26T22:37:00-05:00
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
