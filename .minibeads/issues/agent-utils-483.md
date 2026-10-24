---
title: 'thread-card-read-style: a fully read thread card looks read'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-24T10:13:17.177379786+00:00
updated_at: 2026-10-24T10:13:17.177379786+00:00
---

# Description

[muse watermelon] The owner (phone screenshot, 2026-10-24): thread cards "stay the same font when unread" and a thread whose child messages are all dealt with should be dimmed just like messages. Reading, confirmed in the code: message rows dim when READ (archived/own rows at reduced opacity, desaturated), so a card whose thread has no unread messages gets that same read treatment and a card with unread stays bright. In the screenshot a fully read 1-reply card is as bright as the unread one beside it. Wanted in both card lists (the Threads view cards and the Threads list), consistent with Show/Fold/Hide read and with the existing grey-edge rule under Unpack unread; selected, highlighted and focused cards come back to full strength as read message rows do.

# Acceptance Criteria

Under Show read a fully read card is drawn with the read rows treatment (same opacity and desaturation amounts) and an unread card is unchanged; Fold read still folds runs of read cards and Hide read still leaves them out; selection, highlight and focus restore full strength; the Unpack-unread grey edge still holds; page tests plus a Chromium walk.
