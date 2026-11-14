---
title: 'discuss-summary-cloud-read-404: cloud voice reads the Discuss summary'
status: closed
priority: 1
issue_type: bug
assignee: mu-discussread
labels:
- vibe-talk
created_at: 2026-11-14T15:50:00.000000000+00:00
updated_at: 2026-11-14T18:30:00+00:00
---

# Description

[watermelon-full-preview] Owner UI request 059(a) (2026-11-14 10:47 AM ET). Reading the Discuss summary aloud with cloud voice fails on the owner's phone with the banner "Cloud voice: HTTP 404 unknown_document: no such document. The message was not archived." and "Read with device voice" offered. Device voice works; cloud voice does not.

The Discuss panel (#1501 discuss-read-aloud-parity) hands the summary to the shared read-aloud owner (#1505) as a document whose key is `discuss:<channel>|<thread>|<created_ms>`. The owner's cloud path prepares speech through `POST /api/v1/documents/{key}/speech/prepare`, which only knows keys in the Documents index, so a Discuss key is always answered 404 `unknown_document`. The summary itself is saved server-side in the Discuss record (scope plus `created_ms`), so the server already holds the exact words to speak.

# Design

- In the document speech-prepare route, recognise a Discuss read key, parse its scope and `created_ms`, and look the summary up in the saved Discuss record for that scope: the current summary, a superseded timeline summary with that generation, or the current summary for a not-yet-named generation (0, the fresh-Summarize window before the record redraw). Mint the ticket from that server-saved text exactly as a cached document's ticket is minted; caller-supplied text is never spoken.
- A Discuss key naming no saved summary answers 404 `unknown_discuss_summary`; a genuine document key keeps the existing `unknown_document` answer.
- The cloud failure wording maps those codes to words an owner can act on ("that Discuss summary is no longer saved on the server. Open Discuss, Summarize again, then tap Read", and the reopen wording for a document), without the "The message was not archived" tail, which names an act document and Discuss reads never perform.

# Acceptance Criteria

- Cloud voice reads a Discuss summary through the real server route, including after a page reload and after close/reopen of Discuss (the same saved record and key are re-derived on reopen, so a repeated prepare for the same key answers the same ticket shape).
- A superseded summary in the record timeline can still be prepared by its own generation; a key naming no saved summary fails with `unknown_discuss_summary`, never `unknown_document`.
- The failing banner for a genuine failure says what to do next and never says "The message was not archived" for a document/Discuss read.
- A failing test through the real server route (seeded Discuss record, fake TTS vendor) exists before the fix and passes after it; browser tests at phone (~390 px) and desktop widths cover the visible behaviour; unit tests cover the changed logic.

# Notes

Owner UI fast-track (owner rule 286), lane mu-discussread from coordb. Shared read-aloud owner from #1505, Discuss read-aloud from #1501, per-device summary persistence from #1497 and the server Discuss record from discuss-transcript-persist. Do not touch the Discuss top bar markup (mu-discusstop), the `.discuss-bound` span (mu-hoursbound), `web/voice.js` or `web/src/app/main.js`. Gate: owner tier (GATE_PRIORITY=owner), then land-main and coordb deploys.

## Resolution

Landed as 0dda16f7 on main and deployed to the devbig014 Slack instance, live 11:53 AM ET 2026-11-14 (release a5d56431); the 15-minute post-deploy watch passed. Closed by the devbig014 coordinator.
