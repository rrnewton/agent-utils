---
title: 'vibe-talk-voice-tool-audit: prove every voice-agent tool reads only through the vibe-talk server'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-22T10:25:00-05:00
updated_at: 2026-11-22T10:25:00-05:00
---

# Description

The owner asked to confirm that the voice agent tools only access the vibe-talk server following the state sync model and never go directly to the chat provider.

# Design

List every voice-agent tool (name, what it reads, code path from the voice bridge to the server) and prove from the code that each goes only through the vibe-talk server and its synced state. Any direct provider path is a finding with a fix. Add a test that fails if a voice tool reaches anything but the server.

# Acceptance Criteria

- The audit table is published; a test enforces the server-only rule; any direct path found has a fix or an issue.

# Notes

Filed by the coordinator; the owning lane refines this file.
