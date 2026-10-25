# Evidence 1: why All said "112 messages … all of it is loaded"

2026-10-25, `#521 server-side-search`. Redactions: the channel is "channel A" (its alias in the
owner's page is left out); channel ids, the host and the private shim's location are omitted.

## The channel, as Slack itself lists it (read-only, 2026-10-25 ~22:10 UTC)

`scripts/walk_history.py` over channel A's `conversations.history`:

```
page 1..4 rows 20 each, page 5 rows 2, more=False
{"pages": 5, "messages": 82, "thread_roots": 31, "replies_by_reply_count": 57,
 "oldest_utc": "2026-10-23 23:24", "reached_start": true, "seconds": 7.9,
 "p50_s": 1.59, "max_s": 1.8}
```

So channel A is **two days old** (first message 2026-10-23 23:24 UTC) and holds 82 top-level rows
(two of them system events the server drops: 80 real) and 57 replies in 31 threads: **137 real
messages**. A wildcard search listing (evidence 2) returned 138: the same 137 plus one posted
between the two reads. The private server's message cache held exactly the same 137 (80 in Main,
57 replies), and its Main coverage reaches back past the channel's first message.

The earlier traffic of the same agent is in a different channel ("channel B", whose alias in the
owner's page is now "Offline build"): 33 top-level messages and 183 replies from 2026-10-20 to
2026-10-23. Nothing in channel A is older than 2026-10-23.

## What All can load: replay of the Flat bound over channel A's real shape

`scripts/simulate_flat.py` (the model of `HttpSlackClient::flat`) over channel A's 80 top-level
messages with their reply counts and thread activity, newest first, 50 a page as the page asks:

```
top_level 80, replies 57, channel_total 137, all_view_loads_at_most 114
page 1: 50 top-level, 21 threads, replies loaded 19, replies NEVER loaded 23, notice true
page 2: 30 top-level, 10 threads, replies loaded 15, replies never loaded 0, notice false
```

The owner saw 112: within 2 of 114. The difference is not pinned down (candidates: rows the page
hides as empty, or the two reads being minutes apart).

## Reproduction in the server's own code (not committed to the suite)

A throwaway integration test over the suite's loopback Slack fake (`tests/slack_fake`), run once
on 2026-10-25 and removed again:

```rust
mod slack_fake;

use slack_fake::{profiled, thread, ts, Fake, FakeState};
use vibe_talk::chat::ChatClient;
use vibe_talk::model::ChannelId;
use vibe_talk::threads::{TimelineRequest, TimelineView};

const CH: &str = "C0000MAIN1";

#[tokio::test]
async fn flat_reaches_the_start_without_every_reply() {
    let mut state = FakeState::default();
    let mut list = Vec::new();
    let mut replies_total = 0;
    for i in 0..80_i64 {
        let at = ts(i * 10, 0);
        let mut message = profiled(&at, "UADA00001", "Ada", &format!("message {i}"));
        // The newest 50 (30..80) hold 21 roots; the oldest 30 hold 10.
        let root = if i >= 30 { i <= 50 } else { i % 3 == 0 };
        if root {
            let replies: Vec<(String, String)> = (0..2)
                .map(|r| (ts(10_000 + i * 10 + r, 0), format!("reply {r} to {i}")))
                .collect();
            replies_total += replies.len();
            thread(&mut state, CH, &mut message, &replies);
        }
        list.push(message);
    }
    state.history.insert(CH.to_owned(), list);
    let fake = Fake::start(state, 20).await;
    let client = fake.client();
    let channel = ChannelId(CH.to_owned());
    let mut request = TimelineRequest {
        view: TimelineView::Flat, thread_id: None, before: None, after: None, limit: 50,
    };
    let first = client.fetch_timeline(&channel, &request).await.expect("newest");
    request.before = first.next_before.clone();
    let older = client.fetch_timeline(&channel, &request).await.expect("older");
    let loaded = first.messages.len() + older.messages.len();
    println!("channel: 80 top-level + {replies_total} replies = {}; loaded {loaded} \
              (newest page {} with notice {:?}; older page {} has_more {})",
        80 + replies_total, first.messages.len(), first.notice.is_some(),
        older.messages.len(), older.has_more);
    assert!(!older.has_more, "the start of the channel is reported reached");
    assert!(first.notice.is_some(), "the bound applied, and said so only in the notice");
    assert!(loaded < 80 + replies_total, "yet replies were never loaded");
}
```

Output:

```
channel: 80 top-level + 62 replies = 142; loaded 120 (newest page 70 with notice true; older page 50 has_more false)
test result: ok. 1 passed; 0 failed
```

## The chain of code that turns that into "all of it is loaded"

1. `vibe-talk/src/slack/timeline.rs`, `flat`: the page is Main's page plus the replies of at most
   `FLAT_MAX_THREADS` (10) threads rooted on it; `has_more` and `next_before` are **Main's**
   (`has_more: page.has_more`). The bound is said only in a free-text `notice`.
2. Slack's All is never answered from the message cache (`cache/shadow.rs`: "Slack's Flat view
   never"), so the served-page cursor (`#510`) is not involved.
3. `web/voice.js`, `applyTimelinePage`: `discordMoreAbove = payload.has_more === true` on a step
   back; the store's cover for All takes `more` from the same field (`foldTimelinePage`).
4. `historyReadToStart()` is `discordMoreAbove === false` and the cover's `more === false`;
   `searchEmptySentence` then says "Nothing in All matches that search — all of it is loaded."
5. `channelSummary(count, loadedIsWhole(), label)` says "`<count>` messages from `<label>`", a
   phrasing that reads as the channel's total, whenever `loadedIsWhole()`; the count is the rows
   loaded in the view (less those hidden as empty), not anything the server counted.
6. One tap of "Search before …" walks at most `SEARCH_OLDER_PAGES` (5) pages; here the first step
   back already reached Main's start, so the walk ended with reason `start`.
