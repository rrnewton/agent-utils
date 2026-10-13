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
//! for a read only when they are what the read would bring. They are looked up at the thread's
//! turn, the root included, never copied when it is queued, and checked against the newest summary
//! any page has carried of the thread since: an exact count of at most 50, from a current page,
//! read no earlier than any reply held and with nothing pushed or posted into the thread since,
//! matched exactly by rows that all came from current pages, the newest reply no older than the
//! thread's last activity. What changes while a thread waits (an edit or delete pushed, a reply
//! pushed or posted here, a reply counted by a later page, a delta that removes it) is seen at its
//! turn; a reply added with nothing pushed is not, unless a page counts it first, and no test here
//! claims otherwise. Pages that land out of order are ordered by when their reads began, and a page
//! the provider could not bring up to date never replaces one it could, so a renamed thread rides
//! a delta as the newest current read showed it, and never once a delta has removed it. Plus two
//! queue rules: a channel taken off the list is not read or filed for, and one channel's backlog
//! does not keep another's threads out of the queue.
//!
//! Every test runs on tokio's paused clock, so minutes of budget cost no wall time. Read-ahead
//! keeps its rows by the wall clock, which these tests barely move, so a row never expires in
//! one; the storage purge (`ReadAhead::clear`) stands in for expiry where a test needs rows gone.

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
use vibe_talk::summarize::threads::{
    sentence_key, FiledSentence, ThreadDeps, ThreadNames, ThreadNaming, ThreadRequest,
};
use vibe_talk::summarize::{Summarizer, Summary, SummaryError, SummaryRequest};
use vibe_talk::testing::{self, CountingChat, READ_CHANNEL, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::threads::{
    MessageThread, ThreadSummary, TimelineDelta, TimelinePage, TimelineRequest, TimelineView,
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
    /// When set, the thread list answers as a provider serving what it could not bring up to date
    /// does: complete only up to this instant (`as_of`).
    list_as_of: Mutex<Option<String>>,
    /// When set, the thread list answers from THESE threads, complete only up to the instant given
    /// (`as_of`): a provider that has fallen behind, serving the channel as it was.
    stale_list: Mutex<Option<(String, Vec<ForumThread>)>>,
    /// The same for the Flat view.
    stale_flat: Mutex<Option<(String, Vec<ForumThread>)>>,
    /// When set, the next thread-list read takes its answer as the threads are when it BEGINS, and
    /// then waits for a permit from this before it answers: a read that began before another and
    /// lands after it.
    hold_next_list: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    /// Threads the thread list leaves out, though they can still be read: moved out of the list.
    unlisted: Mutex<Vec<String>>,
    /// When set, a post to a thread arrives upstream and its answer is lost on the way back.
    lose_post_answers: Mutex<bool>,
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
            TimelineView::Threads => {
                let stale = self.stale_list.lock().unwrap().clone();
                let (as_of, mut threads) = match stale {
                    Some((as_of, threads)) => (Some(as_of), threads),
                    None => (self.list_as_of.lock().unwrap().clone(), threads),
                };
                let unlisted = self.unlisted.lock().unwrap().clone();
                threads.retain(|thread| !unlisted.contains(&thread.id));
                let page = TimelinePage {
                    threads: threads.iter().map(Self::summary).collect(),
                    has_threads: true,
                    as_of,
                    ..TimelinePage::default()
                };
                let hold = self.hold_next_list.lock().unwrap().take();
                if let Some(hold) = hold {
                    hold.acquire().await.expect("never closed").forget();
                }
                page
            }
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
                let stale = self.stale_flat.lock().unwrap().clone();
                let (as_of, threads) = match stale {
                    Some((as_of, threads)) => (Some(as_of), threads),
                    None => (None, threads),
                };
                let mut messages: Vec<Message> = threads
                    .iter()
                    .flat_map(|thread| std::iter::once(&thread.root).chain(&thread.replies))
                    .cloned()
                    .collect();
                vibe_talk::model::sort_oldest_first(&mut messages);
                TimelinePage {
                    messages: newest(messages, request.limit),
                    has_threads: true,
                    as_of,
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
    async fn post_in_thread(
        &self,
        channel: &ChannelId,
        thread_id: &str,
        content: &str,
        _reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        let mut posted = None;
        self.change(channel, |threads| {
            if let Some(thread) = threads.iter_mut().find(|thread| thread.id == thread_id) {
                let id = thread
                    .replies
                    .last()
                    .unwrap_or(&thread.root)
                    .id
                    .numeric()
                    .unwrap()
                    + 1;
                let reply = message(channel, id, content, thread_id, false);
                thread.replies.push(reply.clone());
                posted = Some(reply);
            }
        });
        let posted = posted.ok_or_else(|| ChatError::Refused("no such thread".to_owned()))?;
        if *self.lose_post_answers.lock().unwrap() {
            return Err(ChatError::Transport(
                "the connection dropped before the answer".to_owned(),
            ));
        }
        Ok(posted)
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
        &format!(
            "{}\n[ingest]\ntoken = \"{INGEST_TOKEN}\"\n",
            testing::config_toml()
        ),
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

/// What an adapter authenticates its pushes with (`POST /api/v1/live/events`).
const INGEST_TOKEN: &str = "test-ingest-token-0000000000";

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

// --- what changes while a thread waits for its turn ------------------------------------------------
//
// A thread can wait minutes: behind another naming's model call, or for its channel's next read.
// Whatever it is named from is decided when its turn comes, against the newest the server has seen
// of it, never against a copy taken when it was queued. Each test below fails if any part of that
// copy (its root, its reply count, its last activity) is what the naming or a later delta uses.

/// The one prompt that mentions any of `needles`, if there is one.
fn prompt_naming(h: &Harness, needles: &[&str]) -> Option<String> {
    let prompts = h.summarizer.thread_prompts();
    let found: Vec<&String> = prompts
        .iter()
        .filter(|p| needles.iter().any(|needle| p.contains(needle)))
        .collect();
    assert!(found.len() <= 1, "prompts naming {needles:?}: {prompts:#?}");
    found.first().map(|p| (*p).clone())
}

/// Thread `n` of the write channel, as the page names it.
fn write_thread(n: usize) -> String {
    format!("spaces/{WRITE_CHANNEL}/threads/t{n}")
}

/// A forward delta of the write channel's thread list since `cursor`.
async fn thread_delta(h: &Harness, cursor: &str) -> Value {
    get(
        h,
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/timeline?view=threads&after={}",
            urlencode(cursor)
        ),
    )
    .await
}

/// The thread `id` as `page` carries it, if it does.
fn carried(page: &Value, id: &str) -> Option<Value> {
    page["threads"]
        .as_array()
        .expect("a thread list")
        .iter()
        .find(|thread| thread["id"] == id)
        .cloned()
}

#[tokio::test(start_paused = true)]
async fn a_root_edited_and_pushed_while_its_thread_waits_is_named_from_the_new_text() {
    // A thread with no replies is all there in the list's root. The root is edited upstream, and
    // the edit pushed, while the thread waits behind another naming's model call: it is named from
    // the edited text, not from the list's copy of the root taken when it was queued.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 0), 0);
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    let mut root = None;
    h.forum.change(&write_channel(), |threads| {
        threads[1].root.content = "EDITED ROOT of thread 1".to_owned();
        root = Some(threads[1].root.id.clone());
    });
    // What the ingest route does with a pushed update.
    h.state
        .read_ahead
        .forget(&write_channel(), &root.expect("a root"));
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    let prompt = prompt_naming(&h, &["thread 1 starts here", "EDITED ROOT"]).expect("named");
    assert!(
        prompt.contains("EDITED ROOT of thread 1"),
        "named from the root as it was before an edit pushed while it waited: {prompt}"
    );
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "the edited thread is read; the other is named from the list alone"
    );
}

#[tokio::test(start_paused = true)]
async fn a_root_edited_while_its_thread_waits_is_not_named_from_the_list_copy_taken_at_enqueue() {
    // The same with replies: a Flat page and the list held the whole thread when it was queued.
    // Its root is edited and pushed while it waits, and the reader's next Flat page holds the
    // edited root and every reply. Whatever it is named from, it is not the root the list carried.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 2), 0);
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    let mut root = None;
    h.forum.change(&write_channel(), |threads| {
        threads[1].root.content = "EDITED ROOT of thread 1".to_owned();
        root = Some(threads[1].root.id.clone());
    });
    h.state
        .read_ahead
        .forget(&write_channel(), &root.expect("a root"));
    run_for(1).await;
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    let prompt = prompt_naming(&h, &["thread 1 starts here", "EDITED ROOT"]).expect("named");
    assert!(
        prompt.contains("EDITED ROOT of thread 1"),
        "named from the list's root copied at enqueue, not the edited root: {prompt}"
    );
    assert!(prompt.contains("reply 2 in thread 1"), "{prompt}");
}

#[tokio::test(start_paused = true)]
async fn a_thread_deleted_and_pushed_while_it_waits_is_not_named() {
    // Its root deleted upstream, and the delete pushed, while it waits for its turn. Its turn
    // reads it, and the provider says there is no such thread: nothing is sent to the model and
    // nothing is filed.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 0), 0);
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    let mut root = None;
    h.forum.change(&write_channel(), |threads| {
        root = Some(threads.remove(1).root.id);
    });
    h.state
        .read_ahead
        .forget(&write_channel(), &root.expect("a root"));
    h.gate.add_permits(10);
    run_for(5).await;
    assert_eq!(h.names.pending(), 0);
    assert!(
        prompt_naming(&h, &["thread 1 starts here"]).is_none(),
        "a deleted thread's root, pushed as deleted, was sent to the model"
    );
    let filed = h
        .state
        .store
        .cached_summary(&sentence_key(
            &h.state.summary_version,
            &write_channel(),
            &write_thread(1),
        ))
        .await
        .expect("readable");
    assert!(
        filed.is_none(),
        "a sentence filed for a deleted thread: {filed:?}"
    );
    assert_eq!(h.names.named(), 1, "only the thread still there");
}

#[tokio::test(start_paused = true)]
async fn a_thread_deleted_while_it_waits_is_not_put_back_on_the_page_by_a_delta() {
    // A renamed thread rides every forward delta of its list for fifteen minutes, so a page that
    // refreshes by delta sees its name. A page folds those threads in as live ones: one named
    // after it was deleted would be drawn again on every refresh.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 0), 0);
    let list = thread_list(&h, WRITE_CHANNEL).await;
    let mut cursor = list["next_after"].as_str().expect("a cursor").to_owned();
    run_for(1).await;
    let mut root = None;
    h.forum.change(&write_channel(), |threads| {
        root = Some(threads.remove(1).root.id);
    });
    h.state
        .read_ahead
        .forget(&write_channel(), &root.expect("a root"));
    h.gate.add_permits(10);
    run_for(5).await;
    let deleted = write_thread(1);
    for round in 0..3 {
        let delta = thread_delta(&h, &cursor).await;
        let removed = delta["delta"]["removed_threads"]
            .as_array()
            .is_some_and(|removed| removed.iter().any(|t| t == deleted.as_str()));
        assert!(
            carried(&delta, &deleted).is_none() || removed,
            "round {round}: a delta carried the deleted thread as a live one: {delta}"
        );
        cursor = delta["next_after"].as_str().expect("a cursor").to_owned();
        run_for(60).await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_thread_that_grew_while_it_waited_is_not_named_without_its_first_reply() {
    // Three threads of two replies; the third waits a minute for its channel's read. Meanwhile it
    // gains a third reply, and just before its turn the reader's newest Flat page of two rows holds
    // only its replies 2 and 3: as many as the count it was QUEUED with, and as new as the last
    // activity it was queued with. They are not the thread.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 3, 2));
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(5).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "t0 read; t1 and t2 wait"
    );
    let channel = write_channel();
    h.forum.change(&channel, |threads| {
        let thread = &mut threads[2];
        let id = thread.root.id.numeric().unwrap() + 3;
        let thread_id = thread.id.clone();
        thread.replies.push(message(
            &channel,
            id,
            "reply 3 in thread 2",
            &thread_id,
            false,
        ));
    });
    run_for(50).await;
    get(
        &h,
        &format!("/api/v1/channels/{WRITE_CHANNEL}/timeline?view=flat&limit=2"),
    )
    .await;
    run_for(30).await;
    assert_eq!(h.names.named(), 3, "{} pending", h.names.pending());
    let prompt = prompt_about(&h, "thread 2 starts here");
    assert!(
        prompt.contains("reply 1 in thread 2"),
        "named without its first reply, from rows that matched a count taken at enqueue: {prompt}"
    );
    assert!(prompt.contains("reply 3 in thread 2"), "{prompt}");
    assert_eq!(thread_reads(&h, &write_channel()), 3);
}

#[tokio::test(start_paused = true)]
async fn a_renamed_thread_rides_a_delta_with_its_newest_count_not_the_one_it_was_queued_with() {
    // The third thread is queued with one reply and named a minute later. Meanwhile it gains a
    // second, and a full read of the list shows the page two. The delta after that read carries
    // the thread for its new name: with two replies and its newer last activity, or the page
    // would replace what it has with the record as it was when the thread was queued.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 3, 1));
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(5).await;
    let channel = write_channel();
    h.forum.change(&channel, |threads| {
        let thread = &mut threads[2];
        let id = thread.root.id.numeric().unwrap() + 2;
        let thread_id = thread.id.clone();
        thread.replies.push(message(
            &channel,
            id,
            "reply 2 in thread 2",
            &thread_id,
            false,
        ));
    });
    let list = thread_list(&h, WRITE_CHANNEL).await;
    let shown = carried(&list, &write_thread(2)).expect("listed");
    assert_eq!(shown["reply_count"], 2, "{shown}");
    let cursor = list["next_after"].as_str().expect("a cursor").to_owned();
    run_for(70).await;
    assert_eq!(h.names.named(), 3, "{} pending", h.names.pending());
    let delta = thread_delta(&h, &cursor).await;
    let renamed = carried(&delta, &write_thread(2)).expect("a renamed thread rides the delta");
    assert!(renamed["display_name"].is_string(), "{renamed}");
    assert_eq!(
        renamed["reply_count"], 2,
        "the delta carried thread 2 with the count it had when queued: {renamed}"
    );
    assert_eq!(renamed["updated_at"], shown["updated_at"], "{renamed}");
}

#[tokio::test(start_paused = true)]
async fn a_queued_thread_that_grew_and_is_then_opened_is_named_from_that_read_at_once() {
    // The third thread is queued with three replies and gains a fourth while it waits for its
    // channel's read. The reader then opens it: that read holds all four and says four, and it is
    // named from that, now, rather than checked against the three it was queued with, found wanting,
    // and read again at its turn.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 3, 3));
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(5).await;
    assert_eq!(h.names.named(), 1);
    let channel = write_channel();
    h.forum.change(&channel, |threads| {
        let thread = &mut threads[2];
        let id = thread.root.id.numeric().unwrap() + 4;
        let thread_id = thread.id.clone();
        thread.replies.push(message(
            &channel,
            id,
            "reply 4 in thread 2",
            &thread_id,
            false,
        ));
    });
    get(
        &h,
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/timeline?view=thread&thread_id={}",
            urlencode(&write_thread(2))
        ),
    )
    .await;
    run_for(1).await;
    assert_eq!(h.names.named(), 2, "{} pending", h.names.pending());
    assert_eq!(
        thread_reads(&h, &write_channel()),
        2,
        "the first naming read and the reader's own; none for the thread the reader opened"
    );
    let prompt = prompt_about(&h, "thread 2 starts here");
    assert!(prompt.contains("reply 4 in thread 2"), "{prompt}");
}

#[tokio::test(start_paused = true)]
async fn a_sentence_is_filed_at_the_count_its_messages_show_not_the_count_it_was_queued_with() {
    // The third thread is queued with three replies; two are deleted upstream while it waits a
    // minute for its read, and nothing is pushed. Its sentence is written from the root and one
    // reply, and filed as written at two messages, so the schedule asks for the next at four, not
    // at eight.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 3, 3));
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(5).await;
    h.forum.change(&write_channel(), |threads| {
        threads[2].replies.truncate(1);
    });
    run_for(70).await;
    assert_eq!(h.names.named(), 3, "{} pending", h.names.pending());
    let filed = h
        .state
        .store
        .cached_summary(&sentence_key(
            &h.state.summary_version,
            &write_channel(),
            &write_thread(2),
        ))
        .await
        .expect("readable")
        .expect("filed");
    let sentence: FiledSentence = serde_json::from_str(&filed.text).expect("a filed sentence");
    assert_eq!(
        sentence.at, 2,
        "filed at the count the thread had when it was queued: {sentence:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn rows_held_when_a_thread_was_queued_and_gone_by_its_turn_send_it_to_the_provider() {
    // Read-ahead keeps a row for 30 s, and a thread can wait longer than that for its turn. Rows
    // that were there when it was queued and are gone when its turn comes (expired; here, cleared
    // by the storage purge) name nothing, the list's copy of a root included.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 0), 0);
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    h.state.read_ahead.clear();
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "the first was named from the rows it found at its turn; the second found none and was read"
    );
}

#[tokio::test(start_paused = true)]
async fn a_thread_list_the_provider_could_not_bring_up_to_date_names_nothing_without_a_read() {
    // A page that carries `as_of` is complete only up to then: its counts are not what the threads
    // have now, so nothing is named from rows held against them.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 2, 0));
    *h.forum.list_as_of.lock().unwrap() = Some("2026-10-13T00:00:00Z".to_owned());
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(31).await;
    assert_eq!(h.names.named(), 2, "{} pending", h.names.pending());
    assert_eq!(thread_reads(&h, &write_channel()), 2);
}

#[tokio::test(start_paused = true)]
async fn a_thread_a_delta_removes_is_not_carried_as_renamed_nor_named_from_rows_held() {
    // A delta that says a thread left the list is a provider saying so, as a push is.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 3, 0), 1);
    let deps = ThreadDeps::from_state(&h.state);
    let removal = |thread: String| TimelinePage {
        has_threads: true,
        delta: Some(TimelineDelta {
            more: false,
            complete: true,
            deleted: Vec::new(),
            removed_threads: vec![thread],
        }),
        ..TimelinePage::default()
    };
    let delta = TimelineRequest {
        view: TimelineView::Threads,
        thread_id: None,
        before: None,
        after: Some("a cursor".to_owned()),
        limit: 50,
    };
    // Thread 0 is named, from the list alone. Thread 1 is inside its model call, its rows chosen;
    // thread 2 waits for the naming turn.
    let list = thread_list(&h, WRITE_CHANNEL).await;
    let cursor = list["next_after"].as_str().expect("a cursor").to_owned();
    until_named(&h.names, 1).await;
    assert!(carried(&thread_delta(&h, &cursor).await, &write_thread(0)).is_some());

    // A delta removes thread 0, renamed, and thread 2, waiting.
    for thread in [write_thread(0), write_thread(2)] {
        let mut page = removal(thread);
        h.names
            .annotate(
                &deps,
                &write_channel(),
                &delta,
                &mut page,
                std::time::Instant::now(),
                true,
            )
            .await;
        assert!(
            carried(&serde_json::to_value(&page).unwrap(), &write_thread(0)).is_none(),
            "a delta carried a thread it removes as renamed: {page:?}"
        );
    }
    h.gate.add_permits(10);
    until_named(&h.names, 3).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "thread 2, removed while it waited, is named only from a read; thread 1 from the list"
    );

    // Thread 0 is gone upstream as the delta said; a later delta does not put it back.
    h.forum.change(&write_channel(), |threads| {
        threads.remove(0);
    });
    assert!(
        carried(&thread_delta(&h, &cursor).await, &write_thread(0)).is_none(),
        "a later delta put a removed thread back on the page"
    );
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

// --- what the server learns after a count was read, and pages that land out of order ---------------
//
// A count read is only as good as what has happened since. A change the server is TOLD of after
// the count was read (a pushed reply, the owner's own post), rows from a page the provider could not
// bring up to date, a delta saying the thread left the list, and a page that began before another
// but lands after it must each lose to what is newer.

/// Send one request to `app` as `token`, with a JSON body if any.
async fn call(
    app: axum::Router,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Add a reply to thread `n` of the write channel upstream, newer than everything in it.
fn add_reply(h: &Harness, n: usize, content: &str) -> Message {
    let channel = write_channel();
    let mut added = None;
    h.forum.change(&channel, |threads| {
        let thread = &mut threads[n];
        let id = thread
            .replies
            .last()
            .unwrap_or(&thread.root)
            .id
            .numeric()
            .unwrap()
            + 1;
        let thread_id = thread.id.clone();
        let reply = message(&channel, id, content, &thread_id, false);
        thread.replies.push(reply.clone());
        added = Some(reply);
    });
    added.expect("a thread to reply in")
}

/// Push `message` as an adapter does: `kind` is `create` or `update`.
async fn push(h: &Harness, event: &str, kind: &str, message: &Message) {
    let (status, body) = call(
        h.app.clone(),
        INGEST_TOKEN,
        "POST",
        "/api/v1/live/events",
        Some(serde_json::json!({
            "event_id": event,
            "historical": false,
            "kind": kind,
            "message": message,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

/// Hand `page` to naming as the write channel's thread list (a forward delta when `delta`), read
/// by a read that began at `read_began`, as `ops::timeline` does.
async fn annotate_list(
    h: &Harness,
    mut page: TimelinePage,
    delta: bool,
    read_began: std::time::Instant,
) -> TimelinePage {
    let request = TimelineRequest {
        view: TimelineView::Threads,
        thread_id: None,
        before: None,
        after: delta.then(|| "a cursor".to_owned()),
        limit: 50,
    };
    h.names
        .annotate(
            &ThreadDeps::from_state(&h.state),
            &write_channel(),
            &request,
            &mut page,
            read_began,
            true,
        )
        .await;
    page
}

/// A delta of the write channel's thread list saying `thread` left it.
fn removal(thread: String) -> TimelinePage {
    TimelinePage {
        has_threads: true,
        delta: Some(TimelineDelta {
            more: false,
            complete: true,
            deleted: Vec::new(),
            removed_threads: vec![thread],
        }),
        ..TimelinePage::default()
    }
}

/// Start a thread-list read of the write channel that takes its answer now and lands only when
/// the returned semaphore is given a permit.
async fn held_list_read(
    h: &Harness,
) -> (Arc<tokio::sync::Semaphore>, tokio::task::JoinHandle<Value>) {
    let hold = Arc::new(tokio::sync::Semaphore::new(0));
    *h.forum.hold_next_list.lock().unwrap() = Some(Arc::clone(&hold));
    let app = h.app.clone();
    let read = tokio::spawn(async move {
        let (status, body) = call(
            app,
            WRITE_TOKEN,
            "GET",
            &format!("/api/v1/channels/{WRITE_CHANNEL}/timeline?view=threads"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    });
    run_for(1).await;
    assert!(
        h.forum.hold_next_list.lock().unwrap().is_none(),
        "the held read has not begun"
    );
    (hold, read)
}

#[tokio::test(start_paused = true)]
async fn a_reply_pushed_after_the_count_was_read_is_in_the_name() {
    // Two threads of one reply, both held from a Flat page and the list. The first holds the
    // naming turn inside its model call; meanwhile the second gains a reply upstream, and the
    // adapter pushes it. The count the list read no longer says what the thread has, and nothing
    // has read it again: the thread is read at its turn, and the new reply is in its name.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 1), 0);
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    assert_eq!(thread_reads(&h, &write_channel()), 0, "both are held");

    let added = add_reply(&h, 1, "reply 2 in thread 1");
    push(&h, "reply-2", "create", &added).await;
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    let prompt = prompt_about(&h, "thread 1 starts here");
    assert!(
        prompt.contains("reply 2 in thread 1"),
        "named without a reply pushed before its turn: {prompt}"
    );
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "thread 1, read at its turn"
    );
}

#[tokio::test(start_paused = true)]
async fn a_reply_pushed_with_no_thread_label_sends_every_waiting_thread_of_its_channel_to_a_read() {
    // The same, but the adapter's message says nothing of its thread: it may be in any of them,
    // so no count read before it is trusted.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 1), 0);
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);

    let mut added = add_reply(&h, 1, "reply 2 in thread 1");
    added.thread = None;
    push(&h, "reply-2", "create", &added).await;
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    let prompt = prompt_about(&h, "thread 1 starts here");
    assert!(prompt.contains("reply 2 in thread 1"), "{prompt}");
    assert_eq!(thread_reads(&h, &write_channel()), 1);
}

#[tokio::test(start_paused = true)]
async fn a_reply_the_owner_posts_while_its_thread_waits_is_in_the_name() {
    // This server posted it, so it knows: no push needed.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 1), 0);
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);

    let (status, body) = call(
        h.app.clone(),
        WRITE_TOKEN,
        "POST",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/reply"),
        Some(serde_json::json!({
            "text": "the owner's reply in thread 1",
            "thread_id": write_thread(1),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    let prompt = prompt_about(&h, "thread 1 starts here");
    assert!(
        prompt.contains("the owner's reply in thread 1"),
        "named without the owner's own reply: {prompt}"
    );
    assert_eq!(thread_reads(&h, &write_channel()), 1);
}

#[tokio::test(start_paused = true)]
async fn a_reply_the_owner_posted_whose_answer_was_lost_is_in_the_name() {
    // The post reached the thread, and the answer never came back: the reply may be there, so the
    // count read before it is not trusted either.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 1), 0);
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);

    *h.forum.lose_post_answers.lock().unwrap() = true;
    let (status, body) = call(
        h.app.clone(),
        WRITE_TOKEN,
        "POST",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/reply"),
        Some(serde_json::json!({
            "text": "the owner's reply in thread 1",
            "thread_id": write_thread(1),
        })),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    let prompt = prompt_about(&h, "thread 1 starts here");
    assert!(
        prompt.contains("the owner's reply in thread 1"),
        "named without a reply whose post may have arrived: {prompt}"
    );
    assert_eq!(thread_reads(&h, &write_channel()), 1);
}

#[tokio::test(start_paused = true)]
async fn rows_from_a_flat_page_the_provider_could_not_bring_up_to_date_do_not_undo_a_pushed_edit() {
    // A reply is edited upstream and the edit pushed, which drops it from read-ahead. Then the
    // provider, behind, serves a Flat page (with `as_of`) that still holds the reply as it was;
    // its read began after the push, so the push's tombstone does not keep it out. A current
    // thread list follows. The old text must not be what the thread is named from.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 1, 1));
    let before_the_edit = h.forum.threads(&write_channel());
    let mut edited = None;
    h.forum.change(&write_channel(), |threads| {
        let reply = &mut threads[0].replies[0];
        reply.content = "EDITED REPLY in thread 0".to_owned();
        edited = Some(reply.id.clone());
    });
    h.state
        .read_ahead
        .forget(&write_channel(), &edited.expect("a reply"));
    *h.forum.stale_flat.lock().unwrap() =
        Some(("2026-10-13T00:00:00Z".to_owned(), before_the_edit));
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    thread_list(&h, WRITE_CHANNEL).await;
    until_named(&h.names, 1).await;
    let prompt = prompt_about(&h, "thread 0 starts here");
    assert!(
        prompt.contains("EDITED REPLY in thread 0"),
        "named from a stale page's copy of a reply whose edit was pushed: {prompt}"
    );
    assert!(!prompt.contains("reply 1 in thread 0"), "{prompt}");
    assert_eq!(thread_reads(&h, &write_channel()), 1);
}

#[tokio::test(start_paused = true)]
async fn a_root_from_a_page_the_provider_could_not_bring_up_to_date_does_not_undo_a_pushed_edit() {
    // The root of a waiting thread is edited and the edit pushed; a current list holds the edited
    // root, and then a stale Flat page read after it puts the old copy back in read-ahead.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 0), 0);
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    let before_the_edit = h.forum.threads(&write_channel());
    let mut root = None;
    h.forum.change(&write_channel(), |threads| {
        threads[1].root.content = "EDITED ROOT of thread 1".to_owned();
        root = Some(threads[1].root.id.clone());
    });
    h.state
        .read_ahead
        .forget(&write_channel(), &root.expect("a root"));
    thread_list(&h, WRITE_CHANNEL).await;
    *h.forum.stale_flat.lock().unwrap() =
        Some(("2026-10-13T00:00:00Z".to_owned(), before_the_edit));
    get(&h, &flat_page(WRITE_CHANNEL)).await;
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    let prompt = prompt_naming(&h, &["thread 1 starts here", "EDITED ROOT"]).expect("named");
    assert!(
        prompt.contains("EDITED ROOT of thread 1"),
        "named from a stale page's copy of a root whose edit was pushed: {prompt}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_thread_removed_while_its_naming_is_in_flight_is_not_put_back_by_a_delta() {
    // Thread 1 is inside its model call, its rows chosen, when a delta says it left the list (and
    // it has, upstream). Its naming completes; a later delta must not carry it as a live row,
    // which the page would add back.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 0), 1);
    thread_list(&h, WRITE_CHANNEL).await;
    until_named(&h.names, 1).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 1, "thread 1 is being named");
    h.forum.change(&write_channel(), |threads| {
        threads.remove(1);
    });
    annotate_list(
        &h,
        removal(write_thread(1)),
        true,
        std::time::Instant::now(),
    )
    .await;
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;

    let list = thread_list(&h, WRITE_CHANNEL).await;
    let cursor = list["next_after"].as_str().expect("a cursor").to_owned();
    let delta = thread_delta(&h, &cursor).await;
    assert!(
        carried(&delta, &write_thread(1)).is_none(),
        "a delta carried a thread a delta had removed: {delta}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_list_read_before_a_removal_and_landing_after_it_does_not_undo_it() {
    // Thread 1 waits behind thread 0's model call. A delta says it left the list. Then a list
    // whose read began BEFORE that delta lands, still carrying it: it says less than the delta,
    // so thread 1 is still not named from rows held, nor carried on a later delta.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 0), 0);
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    let read_before_the_removal = std::time::Instant::now();
    let listed = TimelinePage {
        threads: h
            .forum
            .threads(&write_channel())
            .iter()
            .map(Forum::summary)
            .collect(),
        has_threads: true,
        ..TimelinePage::default()
    };
    h.forum.change(&write_channel(), |threads| {
        threads.remove(1);
    });
    annotate_list(
        &h,
        removal(write_thread(1)),
        true,
        std::time::Instant::now(),
    )
    .await;
    annotate_list(&h, listed, false, read_before_the_removal).await;
    h.gate.add_permits(10);
    run_for(5).await;
    assert_eq!(h.names.pending(), 0);
    assert!(
        prompt_naming(&h, &["thread 1 starts here"]).is_none(),
        "a thread a delta removed was named from rows held, on an older list's word"
    );
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "thread 1 was read, and the provider said it is gone"
    );
    let list = thread_list(&h, WRITE_CHANNEL).await;
    let cursor = list["next_after"].as_str().expect("a cursor").to_owned();
    assert!(carried(&thread_delta(&h, &cursor).await, &write_thread(1)).is_none());
}

#[tokio::test(start_paused = true)]
async fn a_thread_opened_after_a_delta_removed_it_is_named_from_that_read_and_not_carried() {
    // A delta says thread 1 left the list (it moved out; it can still be read). The reader then
    // opens it: that read began after the removal, so its rows name the thread with no read of
    // naming's own. But a thread page says the thread is there to read, not that it is back in
    // the list, so a later delta still does not carry it.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 0), 0);
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    h.forum.unlisted.lock().unwrap().push(write_thread(1));
    annotate_list(
        &h,
        removal(write_thread(1)),
        true,
        std::time::Instant::now(),
    )
    .await;
    get(
        &h,
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/timeline?view=thread&thread_id={}",
            urlencode(&write_thread(1))
        ),
    )
    .await;
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    assert_eq!(
        thread_reads(&h, &write_channel()),
        1,
        "the reader's own read, and none of naming's"
    );
    let list = thread_list(&h, WRITE_CHANNEL).await;
    assert!(carried(&list, &write_thread(1)).is_none());
    let cursor = list["next_after"].as_str().expect("a cursor").to_owned();
    assert!(
        carried(&thread_delta(&h, &cursor).await, &write_thread(1)).is_none(),
        "a thread page put a thread a delta had removed back on the list"
    );
}

#[tokio::test(start_paused = true)]
async fn an_older_list_landing_late_does_not_roll_back_a_renamed_thread_on_deltas() {
    // Thread 0 is named at one reply. List read A begins and is held. The thread gains a second
    // reply (not yet due for a new sentence), and list read B shows two. Then A lands, with one.
    // A delta after B must carry the thread as B showed it: the page upserts what a delta carries,
    // and a count that goes backwards moves the thread in the list.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 1, 1));
    thread_list(&h, WRITE_CHANNEL).await;
    until_named(&h.names, 1).await;
    let (hold, late) = held_list_read(&h).await;
    add_reply(&h, 0, "reply 2 in thread 0");
    let list = thread_list(&h, WRITE_CHANNEL).await;
    let shown = carried(&list, &write_thread(0)).expect("listed");
    assert_eq!(shown["reply_count"], 2, "{shown}");
    let cursor = list["next_after"].as_str().expect("a cursor").to_owned();
    hold.add_permits(1);
    let landed = late.await.expect("the held read");
    assert_eq!(
        carried(&landed, &write_thread(0)).expect("listed")["reply_count"],
        1,
        "the held read answered as the thread was when it began"
    );
    let delta = thread_delta(&h, &cursor).await;
    let renamed = carried(&delta, &write_thread(0)).expect("a renamed thread rides the delta");
    assert_eq!(
        renamed["reply_count"], 2,
        "an older read landing late rolled the thread back: {renamed}"
    );
    assert_eq!(renamed["updated_at"], shown["updated_at"], "{renamed}");
}

#[tokio::test(start_paused = true)]
async fn a_thread_queued_again_by_an_older_list_landing_late_does_not_roll_back_its_delta_row() {
    // As above, but the older list also queues the thread again: its sentence was swept (here by
    // `forget_channel_sentences`, as taking the channel off the list does) after the newer list
    // landed. When it is named, the row deltas carry is still the newer list's.
    let h = harness(Forum::default().with(WRITE_CHANNEL, 1, 1));
    thread_list(&h, WRITE_CHANNEL).await;
    until_named(&h.names, 1).await;
    let (hold, late) = held_list_read(&h).await;
    add_reply(&h, 0, "reply 2 in thread 0");
    let list = thread_list(&h, WRITE_CHANNEL).await;
    let shown = carried(&list, &write_thread(0)).expect("listed");
    let cursor = list["next_after"].as_str().expect("a cursor").to_owned();
    h.names.forget_channel_sentences(&write_channel());
    hold.add_permits(1);
    late.await.expect("the held read");
    run_for(31).await;
    assert_eq!(h.names.named(), 2, "{} pending", h.names.pending());
    let renamed = carried(&thread_delta(&h, &cursor).await, &write_thread(0)).expect("rides");
    assert_eq!(
        renamed["reply_count"], 2,
        "renamed from an older list, and carried as it said: {renamed}"
    );
    assert_eq!(renamed["updated_at"], shown["updated_at"], "{renamed}");
}

#[tokio::test(start_paused = true)]
async fn an_older_list_landing_late_does_not_undo_what_a_newer_one_said_of_a_waiting_thread() {
    // Thread 1 waits behind thread 0's model call. List read A begins, and is held, while it has
    // no replies; it gains one; list read B shows it; then A lands. Thread 1 is decided at its
    // turn against B: one reply nothing holds, so it is read, and the reply is in its name. Were A
    // taken as the newest, the root alone would match A's count of none.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 0), 0);
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    let (hold, late) = held_list_read(&h).await;
    add_reply(&h, 1, "reply 1 in thread 1");
    let list = thread_list(&h, WRITE_CHANNEL).await;
    let cursor = list["next_after"].as_str().expect("a cursor").to_owned();
    hold.add_permits(1);
    late.await.expect("the held read");
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    let prompt = prompt_about(&h, "thread 1 starts here");
    assert!(
        prompt.contains("reply 1 in thread 1"),
        "named against an older list that landed late: {prompt}"
    );
    let renamed = carried(&thread_delta(&h, &cursor).await, &write_thread(1)).expect("rides");
    assert_eq!(renamed["reply_count"], 1, "{renamed}");
}

#[tokio::test(start_paused = true)]
async fn a_list_the_provider_could_not_bring_up_to_date_does_not_replace_a_current_one() {
    // Thread 1 waits; a current list shows its new reply; then the provider falls behind and serves
    // the list as it was before the reply (`as_of`). Neither while it waits nor once it is named
    // does that list replace what the current one said: a delta carries the thread with its reply.
    let h = gated_harness(Forum::default().with(WRITE_CHANNEL, 2, 0), 0);
    thread_list(&h, WRITE_CHANNEL).await;
    run_for(1).await;
    assert_eq!(h.names.pending(), 2);
    let before_the_reply = h.forum.threads(&write_channel());
    add_reply(&h, 1, "reply 1 in thread 1");
    let list = thread_list(&h, WRITE_CHANNEL).await;
    let shown = carried(&list, &write_thread(1)).expect("listed");
    let cursor = list["next_after"].as_str().expect("a cursor").to_owned();
    let stale = Some(("2026-10-13T00:00:00Z".to_owned(), before_the_reply));
    *h.forum.stale_list.lock().unwrap() = stale.clone();
    thread_list(&h, WRITE_CHANNEL).await;
    *h.forum.stale_list.lock().unwrap() = None;
    h.gate.add_permits(10);
    until_named(&h.names, 2).await;
    let renamed = carried(&thread_delta(&h, &cursor).await, &write_thread(1)).expect("rides");
    assert_eq!(
        renamed["reply_count"], 1,
        "a stale list replaced a current one while the thread waited: {renamed}"
    );
    assert_eq!(renamed["updated_at"], shown["updated_at"], "{renamed}");

    // And once it is named.
    *h.forum.stale_list.lock().unwrap() = stale;
    thread_list(&h, WRITE_CHANNEL).await;
    *h.forum.stale_list.lock().unwrap() = None;
    let renamed = carried(&thread_delta(&h, &cursor).await, &write_thread(1)).expect("rides");
    assert_eq!(
        renamed["reply_count"], 1,
        "a stale list replaced a current one on a renamed thread: {renamed}"
    );
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
