---
title: 'sent-read-waits-for-done: a confirmed send asks the chat service only after its Done is saved'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-14T23:02:09.699471316+00:00
updated_at: 2026-10-15T06:45:14.426323317+00:00
closed_at: 2026-10-15T06:45:14.426323187+00:00
---

# Description

[opus 5.5] Filed from the review of #295 shift-e-read-collapse.

markSentRead (#300 reply-marks-read) goes through markAnsweredDone, which returns early when the answered message is already drawn Done and does not wait on doneSaving. A message is drawn Done the moment it is marked, before its dismissal write is even sent, so a send that confirms while that Done is still being saved can move the chat service's read marker through the message before a Done that then fails. The service's marker cannot be moved back, so the message ends unread here and read in the service.

#295 shift-e-read-collapse closed the same race for Shift+E with doneSaved(ids): it waits for a Done on its way and looks again, asking the service nothing after a Done that failed. A confirmed send should do the same before markReadInService.

Unlikely in practice, because a send takes far longer than a dismissal write; but nothing orders them.
