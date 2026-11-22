---
title: 'vibe-talk-voice-tool-audit: prove every voice-agent tool reads only through the vibe-talk server'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-22T10:25:00-05:00
updated_at: 2026-11-22T11:30:00-05:00
closed_at: 2026-11-22T11:30:00-05:00
---

# Description

The owner asked to confirm that the voice agent tools only access the vibe-talk server following the state sync model and never go directly to the chat provider.

# Design

List every voice-agent tool (name, what it reads, code path from the voice bridge to the server) and prove from the code that each goes only through the vibe-talk server and its synced state. Any direct provider path is a finding with a fix. Add a test that fails if a voice tool reaches anything but the server.

# Acceptance Criteria

- The audit table is published; a test enforces the server-only rule; any direct path found has a fix or an issue.

# Notes

Filed by the coordinator; the owning lane refines this file.

## Resolution

Audit published 2026-11-22 (the owner has the link). The voice agent never reaches the chat provider directly: the deployment's voice bridge sends its credential only to this server's origin and registers only the tools this server lists, and nothing under vibe-talk/src/mcp/ holds the provider client. Five tools (list_channels, search, fetch_document, fetch_document_section, mark_read) touch only server state, and post_reply proposes by default and sends through the post gate. Six read tools (recent_activity, digest_channel, read_page, count_messages, find_message, read_message) are cache-first but fall back to a server-side provider read on a cache miss, and a cached answer starts a background refresh: that fix is #2118 vibe-talk-voice-tools-cache-only.

vibe-talk/tests/voice_tool_boundary.rs enforces the boundary: no MCP module names the provider client; every tool a voice session can call is classified; on a cold cache with serving on, the never-upstream tools make no provider call and each known fallback still makes one, so the fallback list only shrinks, to empty when #2118 lands. Calls are counted, never timed.
