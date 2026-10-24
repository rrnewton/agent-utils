---
title: 'slack-app-deep-link: open Slack messages in the app via slack://, no blank tab, as a Settings choice'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-10-24T04:51:51.783630644+00:00
updated_at: 2026-10-24T06:53:06.917369565+00:00
closed_at: 2026-10-24T06:53:06.917369315+00:00
---

# Description

[muse watermelon] The owner, of opening a Slack link: "that opens Chrome and leaves a blank tab behind at a URL like https://<workspace>.slack.com/archives/C.../p...?thread_ts=...&cid=C... ; I don't know if there's any way to streamline that."

Wanted: investigate Slack's app deep link (slack:// with team/channel/message-thread ids; documented format; what the page knows — team id from auth.test or the workspace URL). Where it helps, open slack:// directly with no tab, keeping the https link for Copy and for no-app case. A Settings choice "Open Slack links in: app / browser", defaulting to whatever works without a blank tab if detection is possible. Google Chat has no such scheme; unchanged. Page tests for the link building and the choice.

# Acceptance Criteria

With the app choice, Open on a Slack message navigates to a slack:// URI carrying team, channel and message (and thread for a reply), no https tab; Copy still copies https; browser choice and non-Slack providers unchanged; team unknown falls back to https said so somewhere visible.
