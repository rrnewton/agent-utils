---
title: 'chat-false-not-sent: the bridge must not report a posted reply as not sent'
status: open
priority: 3
issue_type: task
labels:
- agentctl
created_at: 2026-11-02T16:26:35.736535158+00:00
updated_at: 2026-11-02T16:26:35.736535158+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). bridge told the coordinator 'Chat reply not sent ... block 558' at ~06:56Z although reply 558 was posted at 06:51:47Z (C0CAAAF16QK/messages/1793515909.360369) via transcript text; a resend would have duplicated it. The not-sent notice must check the request's sent ledger first. (2026-11-01)
