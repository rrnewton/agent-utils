---
title: 'chat-settle-uncertain: an operator command to settle a delivery_uncertain chat request'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
created_at: 2026-11-02T18:18:28.744355350+00:00
updated_at: 2026-11-02T18:18:28.744355350+00:00
---

# Description

[opus 5.5] 2026-11-02: a delivery_uncertain request stays stalled forever. The user guide says an operator must settle it, but no command exists (checked builds f0ec02a1 and 1230de55), so chat health can never return to OK without hand-editing state. Six such requests (3 h to 18 d old) sit on one consumer bridge now. Ask: 'agentctl chat settle KEY --as delivered|abandoned' (KEY = request key or reply alias), with a durable record of who settled it, when and why; health lists unsettled ones with the exact command; refuse while the request is mid-delivery. Tests.
