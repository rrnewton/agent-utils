---
title: 'voice-page-optimistic-done: Shift+E, e and Done update locally at once and sync through the outbox'
status: open
priority: 0
issue_type: feature
assignee: cl-sync
labels:
- vibe-talk
created_at: 2026-11-05T15:36:13.750242487+00:00
updated_at: 2026-11-05T15:36:13.750242487+00:00
---

# Description

[opus 5.5] OWNER REQUEST (2026-11-05, 10:27 AM ET). The owner's words, verbatim: "I'm experiencing some latency on pressing shift-e to mark down and collapse the current message. Why? Shouldn't that be a completely local operation with no network round trip? This comes back to our model of local state plus synchronization."

Why it is slow: in `view/shell/readcollapse.ts`, `readAndCollapse` runs `foldForRead` synchronously, then awaits three things before the status line updates:
- `deps.markRowDone`;
- `deps.doneSaved`, a server round trip;
- `deps.markReadInService`, a `POST /upstream-read` to the chat service.

So the visible Done/read state and its confirmation wait on two network round trips.

# Design

- Shift+E, plain `e` and Done become fully optimistic. Local state and the UI (fold, done mark, read state, status line) update immediately, in the same frame.
- The server Done save and the chat-service read marker go through the existing outbox/sync layer, the local-state-plus-synchronization model.
  - Retries are durable.
  - Failures surface quietly (a small badge or toast), and the state is reconciled on sync.
  - Nothing on the keypress path awaits the network.
- Check whether `markRowDone` or the fold triggers an expensive re-render (a whole-list re-render on a large channel), and fix it if so.
- Add keypress-to-paint timing instrumentation, off by default and easy to enable (a debug overlay or a console/performance log). Report before and after numbers.

# Acceptance Criteria

- Test: with a stubbed slow fetch, the fold, done mark, read state and status line update before the network resolves.
- Test: an offline or failed save reconciles through the outbox and surfaces quietly.
- The timing instrumentation exists, and the issue's resolution records keypress-to-paint numbers before and after.
- Gate, land, and send the SHA to the lead for an immediate deploy.

# Notes

Relayed by the lead on 2026-11-05 (owner reply 163). Assigned to cl-sync, a new lane started for it, so that it runs in parallel with cl-refactor's #889/#890.
