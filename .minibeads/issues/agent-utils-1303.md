---
title: 'voice-transcript-share: persist every voice call server-side and share one transcript across devices'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-11T15:08:15+00:00
updated_at: 2026-11-11T15:08:15+00:00
---

# Description

[watermelon-full-preview] Owner report (reply 362, 2026-11-11 10:04 AM ET): "I notice the transcript with the voice agent is not shared between my android and desktop PWA -- is it not stored server side?"

Liaison evidence from the vibe-talk server log on the serving host (2026-11-10 and 2026-11-11): 14 GET /api/v1/voice-socket (calls) but only 3 POST /api/v1/conversations/{id}/turns. Most calls therefore record nothing server-side, so the other device has nothing to restore. Suspects in web/src/view/voice/transcript.ts: recordRecord() returns early unless transcriptRecordsSupported && conversationId, so either the internal voice stack (websocket backend via the relay) often supplies no conversation id, or recordingBroken = true sticks after one failed POST.

# Design

- Confirm the real cause with evidence first: server logs, the code path, and a reproduction. Do not guess between the two suspects.
- Make EVERY call's turns persist server-side. If the backend gives no conversation id, the server mints one per relay ticket or call.
- A transient POST failure retries; it must not set a permanent "not recording" state.
- An open voice transcript view picks up turns recorded on another device: at least on open, and live through the existing event stream if that is cheap. Two devices, one transcript.
- Do not edit web/voice.js (#624 voice-page refactor is in progress elsewhere). Work in web/src modules, the server, and the contract.

# Acceptance Criteria

- Server unit tests cover conversation-id minting and turn persistence for calls whose backend supplies no conversation id.
- Page tests cover retry after a transient POST failure (no permanent recordingBroken state) and cross-device pickup of turns on open (and live via the event stream if cheap).
- A browser walk with two pages (contexts) on one conversation passes if feasible.
- Tests wait on events or conditions with generous timeouts; no wall-clock budgets.

# Notes

P0 owner tier, FAST-TRACK. Owner reply 362. Filed by mu-xdevtx before implementation; the fix lands separately on mu-xdevtx/transcript-share.
