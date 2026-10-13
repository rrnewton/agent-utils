//! `#278 naming-throttle`, remote-ops audit W5: what naming threads in the background costs the
//! chat provider, COUNTED per user-visible action through [`CountingChat`].
//!
//! Naming used to read every due thread from the provider, one after another, paced only by the
//! model. On a bridge that rebuilds a whole space for each read, a thread list of twenty unnamed
//! threads cost twenty rebuilds within the minute, and a provider that was failing was asked again
//! for every other thread of the same channel. These tests pin the three rules that replaced that:
//!
//! - a thread whose rows this server has just read (the list's own root for a thread with no
//!   replies, or a Thread or Flat page read in the last 30 s) is named from them, with no read;
//! - otherwise at most one thread read per channel per 30 s;
//! - after a provider failure (5xx, timeout, rate limit), no naming read of that channel for ten
//!   minutes.
//!
//! Every test runs on tokio's paused clock, so minutes of budget cost no wall time.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::summarize::fake::FakeSummarizer;
use vibe_talk::summarize::threads::ThreadNames;
use vibe_talk::testing::{self, CountingChat, READ_CHANNEL, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::threads::{
    MessageThread, ThreadSummary, TimelinePage, TimelineRequest, TimelineView,
};

// --- a provider with threads --------------------------------------------------------------------

/// One thread: its root and its replies, oldest first.
struct ForumThread {
    id: String,
    root: Message,
    replies: Vec<Message>,
}

/// A threaded provider whose channels each hold a fixed set of threads.
///
/// It answers the Threads, Thread, Flat and Main views as a bridge does: a thread list with each
/// root and an exact reply count; a thread as its root and newest replies; Flat as every message
/// of the channel, each labelled with its thread. It records WHEN each thread was read, in tokio's
/// clock, so a test can see how far apart naming spaced its reads.
#[derive(Default)]
struct Forum {
    channels: BTreeMap<ChannelId, Vec<ForumThread>>,
    thread_reads: Mutex<Vec<(tokio::time::Instant, String)>>,
}

fn message(channel: &ChannelId, id: u64, content: &str, thread: &str, is_root: bool) -> Message {
    Message {
        thread: Some(MessageThread {
            id: thread.to_owned(),
            root_message_id: None,
            is_root,
            reply_count: None,
            reply_count_exact: false,
        }),
        id: MessageId(id.to_string()),
        channel_id: channel.clone(),
        author: "claude-integ".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: jiff::Timestamp::from_second(1_790_000_000 + i64::try_from(id).unwrap())
            .unwrap()
            .to_string(),
        spoken_time: String::new(),
        reply_to: None,
        content: content.to_owned(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        reactions: None,
        heading: None,
    }
}

impl Forum {
    /// Give `channel` `count` threads of `replies` replies each.
    fn with(mut self, channel: &str, count: usize, replies: usize) -> Self {
        let channel = ChannelId(channel.to_owned());
        let base: u64 = if channel.as_str() == WRITE_CHANNEL {
            1_000_000
        } else {
            2_000_000
        };
        let threads = (0..count)
            .map(|n| {
                let id = format!("spaces/{}/threads/t{n}", channel.as_str());
                let first = base + 1_000 * n as u64;
                ForumThread {
                    root: message(
                        &channel,
                        first,
                        &format!("thread {n} starts here"),
                        &id,
                        true,
                    ),
                    replies: (1..=replies)
                        .map(|r| {
                            message(
                                &channel,
                                first + r as u64,
                                &format!("reply {r} in thread {n}"),
                                &id,
                                false,
                            )
                        })
                        .collect(),
                    id,
                }
            })
            .collect();
        self.channels.insert(channel, threads);
        self
    }

    fn threads(&self, channel: &ChannelId) -> &[ForumThread] {
        self.channels.get(channel).map_or(&[], Vec::as_slice)
    }

    fn summary(thread: &ForumThread) -> ThreadSummary {
        ThreadSummary {
            id: thread.id.clone(),
            root: Some(thread.root.clone()),
            title: thread.root.content.clone(),
            reply_count: Some(thread.replies.len() as u64),
            reply_count_exact: true,
            updated_at: thread
                .replies
                .last()
                .unwrap_or(&thread.root)
                .timestamp
                .clone(),
            display_name: None,
            summary: None,
        }
    }

    /// When each thread was read, oldest first.
    fn read_times(&self) -> Vec<tokio::time::Instant> {
        self.thread_reads
            .lock()
            .unwrap()
            .iter()
            .map(|(at, _)| *at)
            .collect()
    }
}

fn newest(mut messages: Vec<Message>, limit: u16) -> Vec<Message> {
    let excess = messages.len().saturating_sub(usize::from(limit));
    messages.drain(..excess);
    messages
}

#[async_trait::async_trait]
impl ChatClient for Forum {
    fn provider_name(&self) -> &str {
        "Google Chat"
    }
    fn supports_threading(&self) -> bool {
        true
    }
    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        Ok(ChatIdentity {
            id: "7".to_owned(),
            username: "Reader".to_owned(),
        })
    }
    async fn fetch_page(
        &self,
        _channel: &ChannelId,
        _limit: u16,
        _before: Option<&MessageId>,
        _after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        Ok(vec![])
    }
    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        let threads = self.threads(channel);
        let page = match request.view {
            TimelineView::Threads => TimelinePage {
                threads: threads.iter().map(Self::summary).collect(),
                has_threads: true,
                ..TimelinePage::default()
            },
            TimelineView::Thread => {
                let id = request.thread_id.clone().unwrap_or_default();
                self.thread_reads
                    .lock()
                    .unwrap()
                    .push((tokio::time::Instant::now(), id.clone()));
                let thread = threads
                    .iter()
                    .find(|thread| thread.id == id)
                    .ok_or_else(|| ChatError::Refused("no such thread".to_owned()))?;
                let mut messages = vec![thread.root.clone()];
                messages.extend(thread.replies.iter().cloned());
                TimelinePage {
                    messages: newest(messages, request.limit),
                    thread: Some(Self::summary(thread)),
                    has_threads: true,
                    ..TimelinePage::default()
                }
            }
            TimelineView::Flat => {
                let mut messages: Vec<Message> = threads
                    .iter()
                    .flat_map(|thread| std::iter::once(&thread.root).chain(&thread.replies))
                    .cloned()
                    .collect();
                vibe_talk::model::sort_oldest_first(&mut messages);
                TimelinePage {
                    messages: newest(messages, request.limit),
                    has_threads: true,
                    ..TimelinePage::default()
                }
            }
            TimelineView::Main => TimelinePage {
                messages: threads.iter().map(|thread| thread.root.clone()).collect(),
                has_threads: true,
                ..TimelinePage::default()
            },
        };
        Ok(page)
    }
    async fn post_message(
        &self,
        _channel: &ChannelId,
        _content: &str,
        _reply: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused("read only".to_owned()))
    }
}

// --- the server ---------------------------------------------------------------------------------

struct Harness {
    app: axum::Router,
    forum: Arc<Forum>,
    chat: Arc<CountingChat>,
    summarizer: Arc<FakeSummarizer>,
    names: Arc<ThreadNames>,
}

fn harness(forum: Forum) -> Harness {
    let summarizer = Arc::new(FakeSummarizer::naming_threads());
    let (mut state, _discord, _store) =
        testing::state_with_backend(&testing::config_toml(), summarizer.clone());
    let forum = Arc::new(forum);
    let chat = Arc::new(CountingChat::new(forum.clone()));
    state.replace_chat(chat.clone());
    Harness {
        app: vibe_talk::http::router(state.clone()),
        names: Arc::clone(&state.thread_names),
        forum,
        chat,
        summarizer,
    }
}

fn write_channel() -> ChannelId {
    ChannelId(WRITE_CHANNEL.to_owned())
}

fn read_channel() -> ChannelId {
    ChannelId(READ_CHANNEL.to_owned())
}

/// What the owner's page asks: a GET with the write token.
async fn get(h: &Harness, path: &str) -> Value {
    let request = Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}

async fn thread_list(h: &Harness, channel: &str) -> Value {
    get(
        h,
        &format!("/api/v1/channels/{channel}/timeline?view=threads"),
    )
    .await
}

/// Every Thread-view read the provider was asked for in `channel`: naming's own, and any a test
/// made itself as the reader.
fn thread_reads(h: &Harness, channel: &ChannelId) -> usize {
    h.chat.timeline_reads(channel, TimelineView::Thread)
}

/// Let the background run for `seconds` of tokio's (paused) clock.
async fn run_for(seconds: u64) {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
}

/// Wait, in tokio's clock, for `count` namings to have completed.
async fn until_named(names: &ThreadNames, count: usize) {
    for _ in 0..1_000 {
        if names.named() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "{count} namings never completed: {} done, {} pending",
        names.named(),
        names.pending()
    );
}

// --- the budget ---------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn twenty_due_threads_cost_at_most_two_thread_reads_a_minute_in_each_channel() {
    // Every thread has replies the list does not carry, and nothing has read them, so each one
    // does need a read. The budget spaces them; it does not drop them.
    let h = harness(
        Forum::default()
            .with(WRITE_CHANNEL, 20, 3)
            .with(READ_CHANNEL, 5, 3),
    );
    thread_list(&h, WRITE_CHANNEL).await;
    thread_list(&h, READ_CHANNEL).await;

    run_for(59).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        2,
        "twenty due threads in one channel, one minute: one read at once and one 30 s later"
    );
    assert_eq!(
        thread_reads(&h, &read_channel()),
        2,
        "the budget is per channel: the other channel's naming is not held behind the first"
    );
    assert_eq!(h.names.named(), 4);

    // Ten minutes later every thread is named, each from exactly one read, at least 30 s apart
    // within a channel.
    run_for(600 - 59).await;
    assert_eq!(h.names.named(), 25, "{} pending", h.names.pending());
    assert_eq!(h.names.pending(), 0);
    assert_eq!(thread_reads(&h, &write_channel()), 20);
    assert_eq!(thread_reads(&h, &read_channel()), 5);
    assert_eq!(h.summarizer.thread_prompts().len(), 25);
    let reads = h.forum.thread_reads.lock().unwrap().clone();
    for channel in [WRITE_CHANNEL, READ_CHANNEL] {
        let times: Vec<_> = reads
            .iter()
            .filter(|(_, thread)| thread.starts_with(&format!("spaces/{channel}/")))
            .map(|(at, _)| *at)
            .collect();
        for pair in times.windows(2) {
            assert!(
                pair[1] - pair[0] >= Duration::from_secs(30),
                "two naming reads of {channel} {:?} apart",
                pair[1] - pair[0]
            );
        }
    }
    assert_eq!(h.forum.read_times().len(), 25);
}

// --- a failing provider -------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_provider_failure_stops_that_channels_naming_reads_for_ten_minutes() {
    let h = harness(
        Forum::default()
            .with(WRITE_CHANNEL, 20, 3)
            .with(READ_CHANNEL, 3, 3),
    );
    h.chat.fail_next(
        "fetch_timeline",
        Some(TimelineView::Thread),
        ChatError::Status {
            status: 502,
            body: "the bridge's scan timed out".to_owned(),
        },
    );
    let failed_at = tokio::time::Instant::now();
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "the 502 was the only naming read: the other 19 threads were not read"
    );
    assert_eq!(h.names.named(), 0);
    assert_eq!(
        h.names.pending(),
        0,
        "nothing waits out the pause holding a place in the queue"
    );

    // The owner keeps reading the list; nothing reads a thread of that channel for ten minutes.
    for at in [60, 300, 599] {
        tokio::time::sleep_until(failed_at + Duration::from_secs(at)).await;
        thread_list(&h, WRITE_CHANNEL).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            thread_reads(&h, &write_channel()),
            1,
            "a naming read of the failing channel {at} s after the failure"
        );
    }

    // Another channel on the same provider is named as usual meanwhile.
    thread_list(&h, READ_CHANNEL).await;
    until_named(&h.names, 1).await;
    assert_eq!(thread_reads(&h, &read_channel()), 1);

    // Ten minutes after the failure, naming reads that channel again.
    tokio::time::sleep_until(failed_at + Duration::from_secs(10 * 60 + 1)).await;
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(thread_reads(&h, &write_channel()), 2);
}

#[tokio::test(start_paused = true)]
async fn a_failure_after_a_success_drops_the_threads_already_waiting_for_a_read() {
    // The budget had the other threads waiting for their turn when the provider began to fail.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 20, 3));
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(thread_reads(&h, &write_channel()), 1);
    assert_eq!(h.names.named(), 1);
    h.chat.fail_next(
        "fetch_timeline",
        Some(TimelineView::Thread),
        ChatError::Transport("the read timed out".to_owned()),
    );
    let failed_at = tokio::time::Instant::now() + Duration::from_secs(29);

    run_for(30).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        2,
        "the second read, at its turn, timed out, and no third followed it"
    );
    assert_eq!(h.names.named(), 1);
    assert_eq!(
        h.names.pending(),
        0,
        "the 18 threads waiting for a read left the queue"
    );

    tokio::time::sleep_until(failed_at + Duration::from_secs(599)).await;
    thread_list(&h, WRITE_CHANNEL).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        2,
        "a naming read of the failing channel inside its ten minutes"
    );
}

// --- rows already read --------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_thread_the_reader_has_just_opened_is_named_from_that_read() {
    let h = harness(Forum::default().with(WRITE_CHANNEL, 1, 3));
    let thread = urlencode(&format!("spaces/{WRITE_CHANNEL}/threads/t0"));
    get(
        &h,
        &format!("/api/v1/channels/{WRITE_CHANNEL}/timeline?view=thread&thread_id={thread}"),
    )
    .await;
    until_named(&h.names, 1).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "the reader's own read, and no second one to name the thread"
    );
    let prompt = &h.summarizer.thread_prompts()[0];
    for line in ["thread 0 starts here", "reply 1 in", "reply 3 in"] {
        assert!(prompt.contains(line), "{line:?} is missing from {prompt}");
    }
}

#[tokio::test(start_paused = true)]
async fn threads_on_a_flat_page_just_read_are_named_from_it() {
    // On a bridge, Flat holds every reply of every thread on it, each labelled with its thread.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 5, 3));
    get(
        &h,
        &format!("/api/v1/channels/{WRITE_CHANNEL}/timeline?view=flat&limit=50"),
    )
    .await;
    thread_list(&h, WRITE_CHANNEL).await;
    until_named(&h.names, 5).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        0,
        "{:?}",
        h.chat.calls()
    );
    for prompt in h.summarizer.thread_prompts() {
        assert!(prompt.contains("reply 3 in"), "{prompt}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_thread_with_no_replies_is_named_from_the_list_alone() {
    let h = harness(Forum::default().with(WRITE_CHANNEL, 20, 0));
    thread_list(&h, WRITE_CHANNEL).await;
    until_named(&h.names, 20).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        0,
        "the list's root is the whole thread"
    );
}

#[tokio::test(start_paused = true)]
async fn a_queued_thread_the_reader_opens_is_named_from_that_read_at_once() {
    let h = harness(Forum::default().with(WRITE_CHANNEL, 3, 3));
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(5).await;
    assert_eq!(thread_reads(&h, &write_channel()), 1, "one read; two wait");
    assert_eq!(h.names.named(), 1);

    // The reader opens the last of the waiting threads: that read names it, now, with no other.
    let thread = urlencode(&format!("spaces/{WRITE_CHANNEL}/threads/t2"));
    get(
        &h,
        &format!("/api/v1/channels/{WRITE_CHANNEL}/timeline?view=thread&thread_id={thread}"),
    )
    .await;
    run_for(1).await;
    assert_eq!(h.names.named(), 2, "{} pending", h.names.pending());
    assert_eq!(
        thread_reads(&h, &write_channel()),
        2,
        "the first naming read and the reader's own; none for the thread the reader opened"
    );

    // The last one waits for its turn in the budget.
    run_for(30).await;
    assert_eq!(h.names.named(), 3);
    assert_eq!(thread_reads(&h, &write_channel()), 3);
}

fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
