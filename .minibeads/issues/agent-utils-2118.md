---
title: 'vibe-talk-voice-tools-cache-only: voice-agent read tools must read only the synced cache, never the chat provider'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- voice
created_at: 2026-11-22T11:00:00-05:00
updated_at: 2026-11-22T11:00:00-05:00
---

# Description

The voice tool audit (#2115) found that six voice-agent read tools (recent activity, digest, page reads, counts, find and read message) are cache-first but not cache-only: on a cache miss the server reads the chat provider itself, every cached answer starts a background provider refresh, and with serving off or a read token every voice read becomes a provider read. This contradicts the owner rule that reading should not require upstream operations; the Discuss agent's equivalent tools are already strictly cache-only.

# Design

Give the voice read tools the Discuss cache-only rule: answer from the synced cache, report a miss honestly instead of fetching, start no background refresh from a tool read. A ratchet test calls every listed tool on a cold cache with a provider fake that records calls and fails if any tool reaches the provider; a source guard keeps the MCP layer from using the provider client. No wall-clock assertions.

# Acceptance Criteria

- The ratchet test passes with an empty known-upstream list; the source guard is in place; voice answers on a cold cache say what is missing rather than fetching.

# Notes

Filed by the coordinator from the #2115 audit.
