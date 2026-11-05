---
title: 'voice-page-optimistic-done: Shift+E, e and Done update locally at once and sync through the outbox'
status: closed
priority: 0
issue_type: feature
assignee: cl-sync
labels:
- vibe-talk
created_at: 2026-11-05T15:36:13.750242487+00:00
updated_at: 2026-11-05T18:27:01.839615310+00:00
closed_at: 2026-11-05T18:27:01.839615199+00:00
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

## Resolution

[opus 5.5 cl-sync] Delivered in two commits.

Part 1: the key path.
- Every Done on a threaded channel rebuilt every row from the store, inside the key's own task. It now dresses the rows where they stand (`renderDoneChange`). Only Hide read, which changes which rows are listed, still rebuilds them.
- Shift+E, e, the Done button and Undo say what they did in the same task as the fold and the Done. The status line no longer says "Saving" for two round trips.
- Shift+E's chat-service answer is said afterwards, and only while the line still shows the words the key put there. A failed or undone write takes those words back, and the error line reports the failure.

Part 2: synchronization.
- Every Done and put-back write is kept on the device until the server answers it (`vibe-talk.inbox-writes`, web/src/core/outbox/inboxwrites.ts).
- A write the server never heard is sent again by itself, in order, with the writes behind it waiting. This covers being offline, a dropped connection, a proxy page, and 408/429/503/504 answers. Retries wait 1 s, then double up to 30 s. The status line says so once, quietly.
- A page closed before the server answered hands its writes to the next page to open, which sends them: only for the same credential (stored as a fingerprint), and only writes under a day old.
- Several pages open on one device each keep their own writes. A page takes another's only once that page has left (`pagehide`) or has gone quiet for 90 s.
- What the server answers and refuses is still taken back, as before.

Key timing: open the page with `?key-timing`, or run `localStorage.setItem("vibe-talk.voice.key-timing", "1")` in the console. Each key then logs `[key-timing] <chord>: handler N ms, paint N ms` to the console and adds a `performance.measure` entry.

Before and after, measured in headless Chromium at 1280x800: a threaded channel of 306 rows, server writes held 400 ms, medians of 6 presses.

| Key | Measure | Before | After |
|---|---|---|---|
| Shift+E | handler | 134 ms | 52 ms |
| Shift+E | key to painted frame | 158 ms | 64 ms |
| Shift+E | status line | 981 ms | 52 ms (in the key's frame; the service's answer follows at about 900 ms) |
| e | handler | 92 ms | 35 ms |
| e | key to painted frame | 105 ms | 45 ms |

Not changed:
- The chat service's read marker (POST /upstream-read) is still asked once, after the Done is saved, and is not retried. The server's floor deduplicates it, the chat service rate-limits marker reads, and a failure is only ever said on the status line.
- A reopened page can draw a left-over Done as unread for one round trip, until its write lands. It then reads the channel again.
- The rest of the handler time is the per-row dress pass over the whole list (`renderChannelRows`), O(rows).
