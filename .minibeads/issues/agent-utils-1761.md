---
title: 'vibe-talk-discuss-99-message-cap: a 211-message thread summarizable as a whole'
status: closed
priority: 0
issue_type: bug
assignee: mu-discusscap
labels:
- vibe-talk
created_at: 2026-11-16T12:00:00+00:00
updated_at: 2026-11-16T12:00:00+00:00
---

# Description

[watermelon-full-preview] Owner, verbatim: "Why is Discuss pane capping me at 99 messages? I have a massive thread of 211 messages from two agents talking to each other that I want to summarize."

The 99 is the server's page size, not a summary or model limit: one page is 99 messages because the chat service's largest page is 100 replies and one slot is kept spare to answer whether an older page exists. Discuss reads only what this server already holds, starting from the newest 99-message page and walking back through the held copy. When the held copy cannot provably be walked further back, the walk stops at the first page, the pane offers exactly 99 unread, and nothing on the pane says older held messages were left out.

# Acceptance Criteria

- A 211-message thread is summarized as a whole: every message is accounted for (cited, or covered by an ordered chunk whose merge says it covers all N messages).
- When the held copy does not reach back over the whole thread, Discuss fetches the missing older pages from the chat service for that thread, paced within the existing rate budget, with progress shown on the pane; the visible "older messages not covered" note is only the fallback when that fetch fails or the budget refuses.
- The pane's count bound is capped by the full thread length, not by the newest held page.
- The summary's message count is checked by a test with a 211-message fixture thread; no message is silently dropped.
- Other Discuss behaviour is unchanged.

# Notes

## Resolution

[watermelon-full-preview] Landed cad769a6 on github/main (gate pass, owner priority). Whole-thread Discuss reads fetch missing older pages through the paced timeline read (one page per preview poll with progress on the pane, to the start on Summarize), summarize over-budget windows in ordered chunks and merge them with citations checked against the whole window, and state the server's own coverage count up front. The count bound caps at the full window length. A refused fetch is the only path to the visible not-covered note. Verified by Rust and panel tests including a 211-message fixture (every message in exactly one chunk, summary count checked) and a nine-image screenshot matrix on a local test instance; thread scope itself could not be rendered against the fake backend (it reports no threading support), so the fetching-progress state is proven by the panel tests with sequenced previews.
