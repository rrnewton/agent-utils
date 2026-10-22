---
title: 'slack-file-share-delivery: a Slack message with an attached file reaches the agent as a request'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
- slack
created_at: 2026-10-22T02:59:43.091097674+00:00
updated_at: 2026-10-22T03:14:46.176172270+00:00
closed_at: 2026-10-22T03:14:46.176172180+00:00
---

# Description

[opus 5.5] On 2026-10-21 the owner posted a thread root with an attached screenshot in the bridged Slack channel. It never reached the agent through the agentctl chat Slack bridge, while his plain-text reply in the same thread did. Neither the subscription plugin's log nor the bridge's mentions the message's ts. Find whether a message carrying a file is published under another event type or dropped by the plugin's filtering, and deliver it as a request, with the file names in the text when the files themselves cannot be passed.

# Acceptance Criteria

A fake-transport test with the captured event shape of a message with a file delivers it as a request naming the file. make validate passes.
