---
title: 'discuss-followup-composer: ask the side agent more, in the panel'
status: open
priority: 0
issue_type: feature
assignee: mu-catchup
labels:
- vibe-talk
created_at: 2026-11-12T19:00:56+00:00
updated_at: 2026-11-12T19:00:56+00:00
---

# Description

[watermelon-full-preview] Owner reply 415 (via coordb): the pane is not a transcript yet — it has no send box to talk to the Discuss agent and continue the conversation. "This is just another messaging surface after all. Probably want the same text box, same send keyboard shortcut... etc."

Parent: #1316 catchup-side-agent. Record: #1345 discuss-transcript-persist.

# Design

- A follow-up composer at the panel's foot reusing the main composer module (same text box, the Enter-sends setting, same shortcuts). Sending appends a `you` turn and the agent's answer to the record's timeline (#1345), persisted server-side per channel/thread.
- The follow-up call carries the same cache-only context (current cached window, #1348) plus the transcript so far — the design's step 5 — in one model call. Still nothing to do with upstream.
- Sent with the pending guard (#1346): no second send while one is in flight.

# Acceptance Criteria

- Type a follow-up, send (button and Enter per the Enter-sends setting): the question and answer appear in the timeline and survive close/reopen on either device profile.
- The model call's context is the cached window plus the transcript; zero provider calls; the turn timestamps/model recorded like a summary's stamp.
- Page tests pin send/Enter/pending; browser walks cover phone 390x844 + desktop; gated/landed per the evening batch.

# Notes

Owner reply 415. Number reserved by coordb. Composer reuse is the main message views' module, not a new text box.
