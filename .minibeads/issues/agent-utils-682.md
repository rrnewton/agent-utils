---
title: 'chat-bridge-uncertain-bounded: resolve delivery_uncertain requests within a bounded time'
status: open
priority: 1
issue_type: bug
created_at: 2026-10-29T12:31:44.238689983+00:00
updated_at: 2026-10-29T12:31:44.238689983+00:00
---

# Description

[watermelon-full-preview] Owner request 2026-10-29 (chat-bridge-robustness batch).

Requests whose prompt delivery became uncertain were left delivery_uncertain forever ("still not typed after 14.4 h"), waiting for an operator to settle them, and the stall log repeated indefinitely.

Wanted: an uncertain request resolves within a bounded time. After half an hour the bridge asks the coordinator queue once more: a processed prompt counts as delivered; a prompt the queue still holds untyped returns to pending and is typed again; a request with a captured reply is delivered (its typing only looked uncertain); anything else is reported once as the bridge's own reply in the channel, recorded on the request, and never typed or reported again.
