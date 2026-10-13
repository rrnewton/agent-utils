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
//! And the limits on the first rule, each counted as the read it does NOT save: held rows stand in
//! for a read only when they are what the read would bring (looked up at the thread's turn, an
//! exact count of at most 50 matched exactly, the newest reply no older than the thread's last
//! activity). Plus two queue rules: a channel taken off the list is not read or filed for, and one
//! channel's backlog does not keep another's threads out of the queue.
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
use vibe_talk::summarize::threads::{sentence_key, ThreadNames, ThreadNaming, ThreadRequest};
use vibe_talk::summarize::{Summarizer, Summary, SummaryError, SummaryRequest};
use vibe_talk::testing::{self, CountingChat, READ_CHANNEL, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::threads::{
    MessageThread, ThreadSummary, TimelinePage, TimelineRequest, TimelineView,
};

// --- a provider with threads --------------------------------------------------------------------

/// One thread: its root and its replies, oldest first.
#[derive(Clone)]
struct ForumThread {
    id: String,
    root: Message,
    replies: Vec<Message>,
}

/// A threaded provider whose channels each hold a set of threads, which a test may change
/// upstream as it goes ([`Forum::change`]), as other people do.
///
/// It answers the Threads, Thread, Flat and Main views as a bridge does: a thread list with each
/// root and an exact reply count; a thread as its root and newest replies (older ones with
/// `before`); Flat as every message of the channel, each labelled with its thread. It records WHEN
/// each thread was read, in tokio's clock, so a test can see how far apart naming spaced its reads.
#[derive(Default)]
struct Forum {
    channels: Mutex<BTreeMap<ChannelId, Vec<ForumThread>>>,
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
            100_000_000
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
        self.channels.get_mut().unwrap().insert(channel, threads);
        self
    }

    fn threads(&self, channel: &ChannelId) -> Vec<ForumThread> {
        self.channels
            .lock()
            .unwrap()
            .get(channel)
            .cloned()
            .unwrap_or_default()
    }

    /// Change `channel`'s threads upstream, as someone else editing, deleting or replying does.
    /// Nothing is pushed to the server: a test that wants a push says so.
    fn change(&self, channel: &ChannelId, edit: impl FnOnce(&mut Vec<ForumThread>)) {
        edit(
            self.channels
                .lock()
                .unwrap()
                .entry(channel.clone())
                .or_default(),
        );
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
                if let Some(before) = request.before.as_deref() {
                    let before: u64 = before.parse().expect("a numeric cursor");
                    messages.retain(|message| message.id.numeric().unwrap() < before);
                }
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
    /// One permit per model call: a test that holds a naming inside its model call takes them
    /// away ([`gated_harness`]).
    gate: Arc<tokio::sync::Semaphore>,
    state: vibe_talk::state::AppState,
}

/// The fake summariser, but each thread naming waits for a permit from `gate` first, so a test
/// can hold one naming inside its model call while others wait for the naming turn.
struct Gated {
    inner: Arc<FakeSummarizer>,
    gate: Arc<tokio::sync::Semaphore>,
}

#[async_trait::async_trait]
impl Summarizer for Gated {
    fn describe(&self) -> &'static str {
        self.inner.describe()
    }
    fn backend(&self) -> &'static str {
        self.inner.backend()
    }
    fn policy_input(&self) -> &str {
        self.inner.policy_input()
    }
    fn names_threads(&self) -> bool {
        self.inner.names_threads()
    }
    async fn name_thread(&self, request: &ThreadRequest<'_>) -> Result<ThreadNaming, SummaryError> {
        self.gate.acquire().await.expect("never closed").forget();
        self.inner.name_thread(request).await
    }
    async fn summarize(&self, request: &SummaryRequest<'_>) -> Result<Summary, SummaryError> {
        self.inner.summarize(request).await
    }
}

fn harness(forum: Forum) -> Harness {
    gated_harness(forum, tokio::sync::Semaphore::MAX_PERMITS)
}

/// A harness whose summariser answers `permits` thread namings, then waits for more
/// (`Harness::gate`).
fn gated_harness(forum: Forum, permits: usize) -> Harness {
    let summarizer = Arc::new(FakeSummarizer::naming_threads());
    let gate = Arc::new(tokio::sync::Semaphore::new(permits));
    let (mut state, _discord, _store) = testing::state_with_backend(
        &testing::config_toml(),
        Arc::new(Gated {
            inner: summarizer.clone(),
            gate: gate.clone(),
        }),
    );
    let forum = Arc::new(forum);
    let chat = Arc::new(CountingChat::new(forum.clone()));
    state.replace_chat(chat.clone());
    Harness {
        app: vibe_talk::http::router(state.clone()),
        names: Arc::clone(&state.thread_names),
        forum,
        chat,
        summarizer,
        gate,
        state,
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
    send(h, "GET", path).await
}

/// What the owner's page sends, with the write token, answered 200.
async fn send(h: &Harness, method: &str, path: &str) -> Value {
    let request = Request::builder()
        .method(method)
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

// --- held rows a fresh read would contradict ------------------------------------------------------
//
// Rows held are a substitute for a read only when they are what the read would bring: a name is
// kept for good, and a sentence until the thread's next milestone. Each test below counts the read
// the held rows did NOT save, because using them would have named the thread from text that is no
// longer there, or without text that is.

fn flat_page(channel: &str) -> String {
    format!("/api/v1/channels/{channel}/timeline?view=flat&limit=50")
}

/// The one prompt that mentions `needle`.
fn prompt_about(h: &Harness, needle: &str) -> String {
    let prompts = h.summarizer.thread_prompts();
    let found: Vec<&String> = prompts.iter().filter(|p| p.contains(needle)).collect();
    assert_eq!(
        found.len(),
        1,
        "prompts mentioning {needle:?}: {prompts:#?}"
    );
    found[0].clone()
}

#[tokio::test(start_paused = true)]
async fn a_reply_deleted_while_its_thread_waits_for_its_turn_sends_the_thread_to_the_provider() {
    // Two threads queued from a Flat page's rows. The first holds the naming turn inside its model
    // call; meanwhile the second's newest reply is deleted upstream and the delete is pushed, which
    // drops that reply from read-ahead. The second is looked up again when its turn comes, finds a
    // reply missing, and is read: not named from a copy taken when it was queued.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 2), 0);
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    assert_eq!(thread_reads(&h, &write_channel()), 0, "both are held");

    let mut deleted = None;
    h.forum.change(&write_channel(), |threads| {
        deleted = threads[1].replies.pop();
    });
    let deleted = deleted.expect("a reply to delete").id;
    // What the ingest route does with a pushed update or delete.
    h.state.read_ahead.forget(&write_channel(), &deleted);
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;

    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "thread 1, read at its turn: {:?}",
        h.chat.calls()
    );
    let prompt = prompt_about(&h, "thread 1 starts here");
    assert!(prompt.contains("reply 1 in thread 1"), "{prompt}");
    assert!(
        !prompt.contains("reply 2 in thread 1"),
        "named from a reply deleted, and pushed, before its turn: {prompt}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_long_thread_opened_at_an_older_page_is_read_for_its_newest_replies() {
    // 120 replies, and the reader scrolls up to the page before reply 71. That page holds fifty
    // replies of the thread, as many as a read would bring (`THREAD_FETCH_LIMIT`), but not the
    // fifty NEWEST.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 1, 120));
    let thread = urlencode(&format!("spaces/{WRITE_CHANNEL}/threads/t0"));
    let reply_71 = 1_000_071;
    get(
        &h,
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/timeline?view=thread&thread_id={thread}\
             &before={reply_71}&limit=50"
        ),
    )
    .await;
    until_named(&h.names, 1).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        2,
        "the reader's older page, and naming's own read of the newest"
    );
    let prompt = &h.summarizer.thread_prompts()[0];
    assert!(prompt.contains("reply 120 in thread 0"), "{prompt}");
}

#[tokio::test(start_paused = true)]
async fn a_reply_deleted_upstream_and_never_pushed_is_not_named_from_when_the_count_says_fewer() {
    // A Flat page holds three replies; one is deleted upstream with no push and no delta, so
    // read-ahead still holds it; the thread list then counts two, exactly.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 1, 3));
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    h.forum.change(&write_channel(), |threads| {
        threads[0].replies.pop();
    });
    thread_list(&h, WRITE_CHANNEL).await;
    until_named(&h.names, 1).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "three held, two counted: the thread is read"
    );
    let prompt = &h.summarizer.thread_prompts()[0];
    assert!(prompt.contains("reply 2 in thread 0"), "{prompt}");
    assert!(
        !prompt.contains("reply 3 in thread 0"),
        "named from a deleted reply: {prompt}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_reply_replaced_after_the_flat_page_is_read_though_the_count_did_not_move() {
    // One reply deleted and another added between the Flat page and the thread list, inside
    // read-ahead's 30 s: still two replies, but the thread's last activity is newer than any reply
    // held, so the held rows are not the thread.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 1, 2));
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    let channel = write_channel();
    h.forum.change(&channel, |threads| {
        let thread = &mut threads[0];
        thread.replies.pop();
        let id = thread.root.id.numeric().unwrap() + 3;
        let thread_id = thread.id.clone();
        thread
            .replies
            .push(message(&channel, id, "a newer reply 3", &thread_id, false));
    });
    thread_list(&h, WRITE_CHANNEL).await;
    until_named(&h.names, 1).await;
    assert_eq!(thread_reads(&h, &write_channel()), 1);
    let prompt = &h.summarizer.thread_prompts()[0];
    assert!(prompt.contains("a newer reply 3"), "{prompt}");
    assert!(!prompt.contains("reply 2 in thread 0"), "{prompt}");
}

// --- the allowlist and the queue ------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_channel_taken_off_the_list_while_its_threads_wait_is_read_no_more() {
    // Threads waiting for the channel's next read can wait minutes. The channel is checked again
    // when each one's turn comes.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 3, 3));
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(thread_reads(&h, &write_channel()), 1);
    assert_eq!(h.names.named(), 1);

    send(&h, "DELETE", &format!("/api/v1/channels/{WRITE_CHANNEL}")).await;
    run_for(90).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "a naming read of a channel taken off the list"
    );
    assert_eq!(h.names.named(), 1);
    assert_eq!(h.names.pending(), 0, "the waiting threads left the queue");
}

#[tokio::test(start_paused = true)]
async fn a_channel_taken_off_the_list_during_a_naming_keeps_no_sentence_from_it() {
    // Taking a channel off the list deletes its sentences. A naming already inside its model call
    // must not file one again afterwards.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 1, 3), 0);
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "read, and now waiting for the model"
    );

    send(&h, "DELETE", &format!("/api/v1/channels/{WRITE_CHANNEL}")).await;
    h.gate.add_permits(1);
    run_for(1).await;
    assert_eq!(
        h.summarizer.thread_prompts().len(),
        1,
        "the model call completed"
    );
    assert_eq!(h.names.pending(), 0);
    assert_eq!(h.names.named(), 0, "nothing was kept from it");
    let filed = h
        .state
        .store
        .cached_summary(&sentence_key(
            &h.state.summary_version,
            &write_channel(),
            &format!("spaces/{WRITE_CHANNEL}/threads/t0"),
        ))
        .await
        .expect("readable");
    assert!(
        filed.is_none(),
        "a sentence filed for a channel taken off the list: {filed:?}"
    );
    assert_eq!(thread_reads(&h, &write_channel()), 1);
}

#[tokio::test(start_paused = true)]
async fn one_channel_with_more_due_threads_than_the_whole_queue_holds_leaves_room_for_another() {
    // More due threads in one channel than the queue holds across every channel (1,024): nine
    // hours of reads at two a minute. They must not hold the queue against another channel's three
    // for those hours, so at most 64 of one channel's threads wait at once.
    let h = harness(
        Forum::default()
            .with(WRITE_CHANNEL, 1_100, 1)
            .with(READ_CHANNEL, 3, 1),
    );
    thread_list(&h, WRITE_CHANNEL).await;
    thread_list(&h, READ_CHANNEL).await;
    assert_eq!(
        h.names.pending(),
        64 + 3,
        "64 of the first channel's threads wait, and all three of the other's"
    );
    run_for(59).await;
    assert_eq!(
        thread_reads(&h, &read_channel()),
        2,
        "the other channel's threads were queued and read on their own budget"
    );
    assert_eq!(thread_reads(&h, &write_channel()), 2);
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
