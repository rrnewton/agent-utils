---
title: 'slack-file-posts: show a line for a Slack post that is only a file'
status: closed
priority: 3
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T04:34:05.349358894+00:00
updated_at: 2026-10-15T04:53:45.505917295+00:00
closed_at: 2026-10-15T04:53:45.505917175+00:00
---

# Description

A Slack message whose content is only a file (files: [...] with empty text) reaches the page as an empty row. Show a file line for it (each file's name and kind, linked to its permalink when Slack gives one), and keep text plus files showing both; never fetch file content. Cover history, Flat, threads and live delivery; test against tests/slack_fake with an image-only post, a text-plus-file post and a file shared into a thread. A compatible Slack bridge may supply only file ids and names: draw what is given.
