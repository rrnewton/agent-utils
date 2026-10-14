---
title: 'reply-in-thinking-block: remind a Claude Code coordinator once of each chat request it never answered'
status: closed
priority: 3
issue_type: feature
labels:
- agentctl
- chat
created_at: 2026-10-14T14:48:59.069331900+00:00
updated_at: 2026-10-14T19:08:49.582977334+00:00
closed_at: 2026-10-14T19:08:49.582976974+00:00
---

# Description

[opus 5.5] Found by the 2026-10-14 branch triage (ai_docs/transient/2026-10-14-branch-triage/REPORT.md). Prior work: commit 67f70db9 on archived/chat-bridge/196-wip-12 (another tracker's #196). On main, a reply the agent writes only inside a thinking block is lost without notice: nothing reminds the agent of a delivered request that got no reply, and the owner sees silence. The later mitigations (chat reply, chat sent, the delivery reaction) do not detect it. Re-implement on the current bridge rather than porting the 4.4k-line diff. Keep its rules: remind once per request; record reminders in reply-reminders.json before queueing; record a start time so older requests are never reminded; a 24 h age limit; withdraw a reminder not typed within 10 s. Find idle state the way inbox/watch.rs already does (claude agents --json matched by HERDR_PANE_ID) rather than adding a second reader; fit the withdraw step to the current delivery path and keep reminders off --deliver-while-working; mention agentctl chat reply in the reminder. These are chat-owned files: coordinate with their owner.

# Acceptance Criteria

An unanswered delivered request produces exactly one reminder once the agent has been idle for the configured time; reminders are recorded before queueing, withdrawn when not typed within 10 s, and never sent for requests older than the bridge's start or 24 h; a test for each rule.
