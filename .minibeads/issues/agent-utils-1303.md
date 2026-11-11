---
title: 'voice-transcript-share: persist every voice call server-side and share one transcript across devices'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-11T15:08:15+00:00
updated_at: 2026-11-11T15:08:15+00:00
closed_at: 2026-11-11T15:08:15+00:00
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

## Resolution

[watermelon-full-preview] Root cause confirmed from the serving host's log (`~/.local/state/vibe-talk-slack/app.log`): 2026-11-10/11 shows 14 voice-socket calls and 3 turn POSTs, including a real 25 s call (1,041 client frames) on 2026-11-10 with zero POSTs. The relayed `vibe-talk-v1` stack does mint a page-local id at socket open, so the missing-id suspect alone does not explain it: in `web/src/view/voice/transcript.ts`, `recordTurn` set `recordingBroken = true` on ANY failed POST and both record paths then silently dropped every later turn for the rest of the page's life — one transient failure in a long-lived PWA page disabled recording on every later call until reload.

Delivered in the closing commit: (1) the server mints `conversation_id` on every `GET /api/v1/voice-session` (`conversation::mint_conversation_id`, beside the relay ticket), the contract is regenerated, and the page files `vibe-talk-v1` turns under the server-minted id, falling back to a local id only with an older server; a hosted vendor's own id still wins. (2) No permanent not-recording state: a transient POST (no answer, or 5xx) is retried at once, the turn is kept and flushed in order ahead of the next turn and at call end; `recordingBroken` now only names an in-progress outage. (3) Cross-device pickup: `refreshStoredConversation` fetches the newest stored page on visibility/focus, appending only turns this page has not seen (identity: conversation, server stamp, speaker, text), so two devices share one transcript without duplicates. Live push through an event stream was not cheap — no transcript event stream exists — so pickup is on open/focus, as the criteria allow.

Tests: Rust unit test for minting, `voice_relay` integration test (two calls mint distinct ids; a turn filed under a minted id reads back), contract tests, and three page tests (transient-failure retry and in-order flush; server-minted id filing; cross-device pickup without duplication, driven through two pages on one store — the two-context browser walk's equivalent at the page layer; a real-browser two-context walk was not run). Unverified: no live call on the deployed server was made from this lane; the coordinator deploys and the owner's next call is the live proof.
