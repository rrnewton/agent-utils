//! remote-ops-audit W5 (naming-budget): how often naming a thread reads the chat provider.
//!
//! A thread is named only from a read of it made at its turn. These tests are about WHEN and HOW
//! OFTEN those reads happen: at most one per channel per thirty seconds by default, newest activity
//! first, none in a channel for ten minutes after a provider failure there, none from a read token,
//! and no queue that grows without bound, waits for ever, or outlives its channel.
//!
//! Every test runs on a paused clock, through the HTTP routes, with every upstream call counted by
//! [`CountingChat`]. Nothing here reaches a model provider or a chat service.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tokio::time::Instant;
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::state::AppState;
use vibe_talk::summarize::fake::FakeSummarizer;
use vibe_talk::summarize::threads::{
    name_key, ThreadDeps, ThreadNames, ThreadRequest, CHANNEL_PAUSE, MAX_QUEUED,
    MAX_QUEUED_PER_CHANNEL, MAX_QUEUED_WAIT, NAMING_READ_TIMEOUT,
};
use vibe_talk::testing::{
    self, CountingChat, READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN,
};
use vibe_talk::threads::{ThreadSummary, TimelinePage, TimelineRequest, TimelineView};

// --- an upstream a test can edit ----------------------------------------------------------------

/// One thread as the chat service holds it.
#[derive(Clone)]
struct FakeThread {
    id: String,
    root: String,
    replies: Vec<String>,
    /// Its last activity, in minutes after 08:00.
    active_minute: u32,
    deleted: bool,
}

/// A chat service of threads in several channels, which a test can edit, delete and hold.
#[derive(Default)]
struct Upstream {
    channels: Mutex<BTreeMap<String, Vec<FakeThread>>>,
    /// A read of one of these threads waits for a permit; one never given never answers.
    holds: Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
    /// Every thread read answered, in order: the thread, and its messages root first, as a name
    /// must be made from them.
    returned: Mutex<Vec<(String, Vec<Message>)>>,
    /// Answer a read of a thread that is gone with an empty page rather than a 404.
    gone_reads_empty: Mutex<bool>,
}

fn thread_id(channel: &str, n: usize) -> String {
    format!("spaces/{channel}/threads/t{n:02}")
}

fn message(channel: &str, id: &str, content: &str) -> Message {
    Message {
        thread: None,
        id: MessageId(id.to_owned()),
        channel_id: ChannelId(channel.to_owned()),
        author: "claude-integ".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: "2026-10-13T08:00:00Z".to_owned(),
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

fn summary_of(channel: &str, thread: &FakeThread) -> ThreadSummary {
    let minute = thread.active_minute;
    ThreadSummary {
        id: thread.id.clone(),
        root: Some(message(
            channel,
            &format!("{}-root", thread.id),
            &thread.root,
        )),
        title: thread.root.clone(),
        reply_count: Some(thread.replies.len() as u64),
        reply_count_exact: true,
        updated_at: format!("2026-10-13T{:02}:{:02}:00Z", 8 + minute / 60, minute % 60),
        display_name: None,
        summary: None,
    }
}

impl Upstream {
    /// Add `count` threads to `channel`, each active after every thread already there.
    fn add(&self, channel: &str, count: usize) -> Vec<String> {
        let mut channels = self.channels.lock().unwrap();
        let threads = channels.entry(channel.to_owned()).or_default();
        let mut added = Vec::new();
        for _ in 0..count {
            let n = threads.len();
            let id = thread_id(channel, n);
            threads.push(FakeThread {
                id: id.clone(),
                root: format!("root of thread {n}: the build is red"),
                replies: vec![
                    format!("reply 0 in thread {n}"),
                    format!("reply 1 in thread {n}"),
                ],
                active_minute: u32::try_from(n).unwrap(),
                deleted: false,
            });
            added.push(id);
        }
        added
    }

    /// Change one thread as somebody upstream would.
    fn change(&self, thread: &str, change: impl FnOnce(&mut FakeThread)) {
        let mut channels = self.channels.lock().unwrap();
        let found = channels
            .values_mut()
            .flat_map(|threads| threads.iter_mut())
            .find(|candidate| candidate.id == thread)
            .expect("a known thread");
        change(found);
    }

    /// Make reads of `thread` wait until the returned gate is given a permit.
    fn hold(&self, thread: &str) -> Arc<tokio::sync::Semaphore> {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        self.holds
            .lock()
            .unwrap()
            .insert(thread.to_owned(), Arc::clone(&gate));
        gate
    }

    fn returned(&self) -> Vec<(String, Vec<Message>)> {
        self.returned.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ChatClient for Upstream {
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
        let channel = channel.as_str();
        match request.view {
            TimelineView::Threads => {
                let threads = self
                    .channels
                    .lock()
                    .unwrap()
                    .get(channel)
                    .cloned()
                    .unwrap_or_default();
                Ok(TimelinePage {
                    // Oldest activity first, as every thread list is.
                    threads: threads
                        .iter()
                        .filter(|thread| !thread.deleted)
                        .map(|thread| summary_of(channel, thread))
                        .collect(),
                    has_threads: true,
                    ..TimelinePage::default()
                })
            }
            TimelineView::Thread => {
                let id = request.thread_id.clone().unwrap_or_default();
                let gate = self.holds.lock().unwrap().get(&id).cloned();
                if let Some(gate) = gate {
                    let _permit = gate.acquire().await;
                }
                let found = self
                    .channels
                    .lock()
                    .unwrap()
                    .get(channel)
                    .and_then(|threads| {
                        threads
                            .iter()
                            .find(|thread| thread.id == id && !thread.deleted)
                            .cloned()
                    });
                let Some(thread) = found else {
                    if *self.gone_reads_empty.lock().unwrap() {
                        return Ok(TimelinePage::default());
                    }
                    return Err(ChatError::Status {
                        status: 404,
                        body: "no such thread".to_owned(),
                    });
                };
                let summary = summary_of(channel, &thread);
                let replies: Vec<Message> = thread
                    .replies
                    .iter()
                    .enumerate()
                    .map(|(n, text)| message(channel, &format!("{id}-reply-{n}"), text))
                    .collect();
                let mut named = vec![summary.root.clone().expect("a root")];
                named.extend(replies.iter().cloned());
                self.returned.lock().unwrap().push((id, named));
                Ok(TimelinePage {
                    messages: replies,
                    thread: Some(summary),
                    has_threads: true,
                    ..TimelinePage::default()
                })
            }
            _ => Ok(TimelinePage::default()),
        }
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

// --- the server around it -----------------------------------------------------------------------

struct Harness {
    app: axum::Router,
    state: AppState,
    upstream: Arc<Upstream>,
    counting: Arc<CountingChat>,
    summarizer: Arc<FakeSummarizer>,
    names: Arc<ThreadNames>,
    start: Instant,
}

/// A server whose thread names are paced as shipped: [`ThreadNames::new`], the default budget.
fn harness() -> Harness {
    harness_with(ThreadNames::new())
}

fn harness_with(names: ThreadNames) -> Harness {
    let summarizer = Arc::new(FakeSummarizer::naming_threads());
    let (mut state, _discord, _store) =
        testing::state_with_backend(&testing::config_toml(), summarizer.clone());
    let upstream = Arc::new(Upstream::default());
    let counting = Arc::new(CountingChat::new(upstream.clone()));
    state.replace_chat(counting.clone());
    state.thread_names = Arc::new(names);
    Harness {
        app: vibe_talk::http::router(state.clone()),
        names: Arc::clone(&state.thread_names),
        state,
        upstream,
        counting,
        summarizer,
        start: Instant::now(),
    }
}

impl Harness {
    /// Let the clock run to `ms` milliseconds after the test began, and everything due by then.
    async fn at_ms(&self, ms: u64) {
        tokio::time::sleep_until(self.start + Duration::from_millis(ms)).await;
    }

    async fn at(&self, seconds: u64) {
        self.at_ms(seconds * 1_000).await;
    }

    async fn send(&self, method: &str, path: &str, token: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Read `channel`'s thread list as the page does, with `token`.
    async fn list(&self, channel: &str, token: &str) -> Value {
        let (status, body) = self
            .send(
                "GET",
                &format!("/api/v1/channels/{channel}/timeline?view=threads"),
                token,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    /// The threads read in `channel` to name them, in order.
    fn named_reads(&self, channel: &str) -> Vec<String> {
        let channel = ChannelId(channel.to_owned());
        self.counting
            .calls()
            .into_iter()
            .filter(|call| {
                call.method == "fetch_timeline"
                    && call.channel.as_ref() == Some(&channel)
                    && call.view == Some(TimelineView::Thread)
            })
            .map(|call| call.thread.unwrap_or_default())
            .collect()
    }

    fn thread_reads(&self, channel: &str) -> usize {
        self.counting
            .timeline_reads(&ChannelId(channel.to_owned()), TimelineView::Thread)
    }
}

fn bad_gateway() -> ChatError {
    ChatError::Status {
        status: 502,
        body: "bad gateway".to_owned(),
    }
}

fn thread_ids(body: &Value) -> Vec<String> {
    body["threads"]
        .as_array()
        .expect("threads")
        .iter()
        .map(|thread| thread["id"].as_str().unwrap().to_owned())
        .collect()
}

fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

// --- (b) the per-channel budget -----------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn twenty_threads_falling_due_over_a_minute_cost_at_most_two_thread_reads() {
    // The page polls the thread list every 2.9 seconds, and a new thread is there each time:
    // twenty due threads in 55 seconds. Before W5 each was read the moment it was queued, twenty
    // reads, each of them up to a whole space's scan on a bridge.
    let h = harness();
    for n in 0..20 {
        h.at_ms(n * 2_900).await;
        h.upstream.add(WRITE_CHANNEL, 1);
        let body = h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
        assert_eq!(thread_ids(&body).len(), usize::try_from(n).unwrap() + 1);
    }
    h.at_ms(59_900).await;
    let reads = h.named_reads(WRITE_CHANNEL);
    assert!(
        reads.len() <= 2,
        "{} thread reads in a minute: {reads:?}",
        reads.len()
    );
    // One at once, and one thirty seconds later: the thread with the newest activity then, which
    // is the one listed at 29.0 seconds, not the oldest still waiting.
    assert_eq!(
        reads,
        [thread_id(WRITE_CHANNEL, 0), thread_id(WRITE_CHANNEL, 10)],
        "not one read at once and then the newest"
    );
    assert_eq!(h.names.named(), 2);
    assert_eq!(h.summarizer.thread_prompts().len(), 2);
    assert_eq!(
        h.counting.count("fetch_timeline"),
        20 + 2,
        "twenty list reads by the page, two thread reads for names, and nothing else"
    );
}

#[tokio::test(start_paused = true)]
async fn every_name_is_made_from_exactly_what_the_read_at_its_turn_returned() {
    // Upstream changes the older thread after it was queued and before its turn: its root and a
    // reply are edited, and a page then shows a new reply. The name is made from the read at its
    // turn, so it holds all three, and nothing of the text the queueing page held.
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 2);
    let (older, newer) = (ids[0].clone(), ids[1].clone());
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(1).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        std::slice::from_ref(&newer),
        "the newer thread first, and the older one waits"
    );

    h.upstream.change(&older, |thread| {
        thread.root = "EDITED ROOT: the build is green again".to_owned();
        thread.replies[1] = "EDITED REPLY: it was the cache key".to_owned();
    });
    h.at(10).await;
    h.upstream.change(&older, |thread| {
        thread
            .replies
            .push("A THIRD REPLY, shown by a later page".to_owned());
        thread.active_minute = 30;
    });
    let page = h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    assert!(thread_ids(&page).contains(&older));
    h.at(20).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL).len(),
        1,
        "a waiting thread that a page showed changed was read before its turn"
    );

    h.at(31).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [newer.clone(), older.clone()],
        "each thread is read once, at its turn: the changed one kept its place"
    );
    let prompts = h.summarizer.thread_prompts();
    let returned = h.upstream.returned();
    assert_eq!(prompts.len(), 2);
    assert_eq!(returned.len(), 2);
    for ((thread, messages), prompt) in returned.iter().zip(&prompts) {
        assert_eq!(
            prompt,
            ThreadRequest::new(messages, 160).prompt(),
            "{thread} was not named from exactly what the read at its turn returned"
        );
    }
    let named = &prompts[1];
    for fresh in ["EDITED ROOT", "EDITED REPLY", "A THIRD REPLY"] {
        assert!(named.contains(fresh), "{fresh} is missing: {named}");
    }
    for stale in ["root of thread 0", "reply 1 in thread 0"] {
        assert!(!named.contains(stale), "{stale} reached the name: {named}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_thread_deleted_while_queued_is_not_named_and_never_reappears_on_a_delta() {
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 2);
    let (older, newer) = (ids[0].clone(), ids[1].clone());
    let first = h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    let after = first["next_after"]
        .as_str()
        .expect("a forward cursor")
        .to_owned();
    h.at(1).await;
    h.upstream.change(&older, |thread| thread.deleted = true);

    h.at(31).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [newer.clone(), older.clone()],
        "the deleted thread is read at its turn, and only then"
    );
    assert_eq!(h.names.named(), 1, "the deleted thread was named");
    assert_eq!(h.summarizer.thread_prompts().len(), 1);
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    assert!(h
        .state
        .store
        .cached_summary(&name_key(&channel, &older))
        .await
        .expect("readable")
        .is_none());

    let (status, delta) = h
        .send(
            "GET",
            &format!(
                "/api/v1/channels/{WRITE_CHANNEL}/timeline?view=threads&after={}",
                urlencode(&after)
            ),
            WRITE_TOKEN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{delta}");
    assert!(delta["delta"].is_object(), "this is a delta: {delta}");
    let carried = thread_ids(&delta);
    assert!(
        carried.contains(&newer),
        "the renamed thread rides the delta: {delta}"
    );
    assert!(
        !carried.contains(&older),
        "a thread deleted while queued came back on a delta: {delta}"
    );

    // And nothing queues it again.
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(120).await;
    assert_eq!(h.thread_reads(WRITE_CHANNEL), 2);
    assert_eq!(h.names.pending(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_renamed_thread_deleted_while_queued_again_leaves_every_delta() {
    renamed_then_deleted_while_queued(false).await;
}

#[tokio::test(start_paused = true)]
async fn a_renamed_thread_whose_read_comes_back_empty_leaves_every_delta_too() {
    renamed_then_deleted_while_queued(true).await;
}

/// Named once, and so carried on deltas as recently renamed; then due again, queued, and deleted
/// before its turn. The read at its turn says it is gone — with a 404, or with `gone_reads_empty`
/// an answer holding nothing at all — and from then on no delta carries it.
async fn renamed_then_deleted_while_queued(gone_reads_empty: bool) {
    let h = harness();
    *h.upstream.gone_reads_empty.lock().unwrap() = gone_reads_empty;
    let ids = h.upstream.add(WRITE_CHANNEL, 2);
    let (older, newer) = (ids[0].clone(), ids[1].clone());
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(31).await;
    assert_eq!(h.names.named(), 2, "both named, at 0 and at 30 seconds");
    let page = h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    let after = page["next_after"]
        .as_str()
        .expect("a forward cursor")
        .to_owned();

    // A fourth message is due a fresh sentence (1, 2, 4, 8...). The channel's budget runs until
    // 60 seconds, so it waits.
    h.at(40).await;
    h.upstream
        .change(&older, |thread| thread.replies.push("one more".to_owned()));
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(45).await;
    assert_eq!(h.names.queued(&ChannelId(WRITE_CHANNEL.to_owned())), 1);
    h.upstream.change(&older, |thread| thread.deleted = true);

    h.at(61).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [newer.clone(), older.clone(), older.clone()]
    );
    assert_eq!(h.names.named(), 2, "the deleted thread was named again");
    let (status, delta) = h
        .send(
            "GET",
            &format!(
                "/api/v1/channels/{WRITE_CHANNEL}/timeline?view=threads&after={}",
                urlencode(&after)
            ),
            WRITE_TOKEN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{delta}");
    assert!(delta["delta"].is_object(), "this is a delta: {delta}");
    assert!(
        !thread_ids(&delta).contains(&older),
        "a thread found gone at its turn is still carried as recently renamed: {delta}"
    );
}

// --- (c) a provider failure pauses the channel --------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_provider_failure_stops_naming_that_channel_for_ten_minutes_then_naming_resumes() {
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 5);
    h.counting
        .fail_next("fetch_timeline", Some(TimelineView::Thread), bad_gateway());
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(1).await;
    assert_eq!(h.named_reads(WRITE_CHANNEL), [ids[4].clone()], "the 502");
    assert_eq!(h.names.named(), 0);

    // The page goes on polling, and the other four threads are due all along. Before W5 only the
    // thread that failed was left alone, and the failing provider was asked for each of the others
    // at once.
    let pause = CHANNEL_PAUSE.as_secs();
    let mut at = 45;
    while at < pause {
        h.at(at).await;
        h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
        at += 45;
    }
    h.at(pause - 1).await;
    assert_eq!(
        h.thread_reads(WRITE_CHANNEL),
        1,
        "a channel whose provider failed was read for naming inside the pause"
    );
    assert_eq!(h.names.named(), 0);

    h.at(pause + 1).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [ids[4].clone(), ids[3].clone()],
        "naming did not resume, newest first, when the pause ended"
    );
    assert_eq!(h.names.named(), 1);
    h.at(pause + 31).await;
    assert_eq!(
        h.thread_reads(WRITE_CHANNEL),
        3,
        "and goes on at its budget"
    );
}

#[tokio::test(start_paused = true)]
async fn a_naming_read_that_never_answers_is_a_timeout_and_pauses_the_channel() {
    // Before W5 a read that never answered held the one naming turn for ever: no thread in any
    // channel was named again.
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 3);
    let _never = h.upstream.hold(&ids[2]);
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(1).await;
    assert_eq!(h.thread_reads(WRITE_CHANNEL), 1);

    let ends = NAMING_READ_TIMEOUT.as_secs() + CHANNEL_PAUSE.as_secs();
    let mut at = 60;
    while at < ends {
        h.at(at).await;
        h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
        at += 60;
    }
    h.at(ends - 1).await;
    assert_eq!(
        h.thread_reads(WRITE_CHANNEL),
        1,
        "the channel was read again before the timeout and its pause had run"
    );
    h.at(ends + 1).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [ids[2].clone(), ids[1].clone()]
    );
    assert_eq!(h.names.named(), 1, "naming never resumed");
}

#[tokio::test(start_paused = true)]
async fn one_channels_pause_leaves_every_other_channel_naming() {
    let h = harness();
    h.upstream.add(WRITE_CHANNEL, 3);
    h.upstream.add(READ_CHANNEL, 3);
    h.counting
        .fail_next("fetch_timeline", Some(TimelineView::Thread), bad_gateway());
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at_ms(100).await;
    assert_eq!(h.thread_reads(WRITE_CHANNEL), 1, "the 502");

    h.list(READ_CHANNEL, WRITE_TOKEN).await;
    for at in [45, 90] {
        h.at(at).await;
        h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
        h.list(READ_CHANNEL, WRITE_TOKEN).await;
    }
    h.at(100).await;
    assert_eq!(
        h.thread_reads(READ_CHANNEL),
        3,
        "the other channel went on at its own budget: at 0.1, 30.1 and 60.1 seconds"
    );
    assert_eq!(
        h.thread_reads(WRITE_CHANNEL),
        1,
        "the failing channel was read again"
    );
    assert_eq!(h.names.named(), 3);
}

// --- read scope ---------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_read_token_queues_nothing_and_moves_nothing_already_queued() {
    let h = harness();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let ids = h.upstream.add(WRITE_CHANNEL, 3);
    for at in [0, 20, 40] {
        h.at(at).await;
        h.list(WRITE_CHANNEL, READ_TOKEN).await;
    }
    h.at(50).await;
    assert_eq!(h.thread_reads(WRITE_CHANNEL), 0);
    assert_eq!(h.names.pending(), 0);

    // The owner's read queues all three: one is read at once, two wait their turn.
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(51).await;
    assert_eq!(h.named_reads(WRITE_CHANNEL), [ids[2].clone()]);
    assert_eq!(h.names.queued(&channel), 2);

    // A new thread, the newest of all, seen only by a read token: not queued, and not read ahead of
    // the two the owner's read queued.
    let unseen = h.upstream.add(WRITE_CHANNEL, 1);
    h.list(WRITE_CHANNEL, READ_TOKEN).await;
    h.at(52).await;
    assert_eq!(h.names.queued(&channel), 2, "a read token queued a thread");
    h.at(111).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [ids[2].clone(), ids[1].clone(), ids[0].clone()]
    );
    h.at(300).await;
    assert!(!h.named_reads(WRITE_CHANNEL).contains(&unseen[0]));
    assert_eq!(h.names.pending(), 0);
    assert_eq!(h.names.named(), 3);
}

// --- bounds -------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_channels_queue_holds_its_newest_threads_up_to_its_cap() {
    let h = harness();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let first = h.upstream.add(WRITE_CHANNEL, 1);
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(1).await;
    assert_eq!(
        h.thread_reads(WRITE_CHANNEL),
        1,
        "the budget now runs to 30s"
    );

    // Eight more than the cap fall due at once.
    let flood = h.upstream.add(WRITE_CHANNEL, MAX_QUEUED_PER_CHANNEL + 8);
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    assert_eq!(h.names.queued(&channel), MAX_QUEUED_PER_CHANNEL);
    assert_eq!(h.names.pending(), MAX_QUEUED_PER_CHANNEL);

    // Worked newest first at the budget until empty. The eight with the oldest activity were the
    // ones left out, for a later read to queue.
    let drained = 30 * (MAX_QUEUED_PER_CHANNEL as u64 + 1);
    h.at(drained).await;
    let mut expected = first.clone();
    expected.extend(flood.iter().rev().take(MAX_QUEUED_PER_CHANNEL).cloned());
    assert_eq!(h.named_reads(WRITE_CHANNEL), expected);
    assert_eq!(h.names.pending(), 0);
}

#[tokio::test(start_paused = true)]
async fn the_queues_together_hold_at_most_the_global_cap() {
    // Ten channels of forty due threads each: more than a channel's cap in every channel, and more
    // than the global cap in all. Every read is held, so the one thread being named holds the turn
    // and nothing else is taken while the queues are counted.
    let h = harness();
    let names = Arc::new(ThreadNames::new());
    let deps = ThreadDeps {
        chat: h.counting.clone(),
        ..ThreadDeps::from_state(&h.state)
    };
    let channels: Vec<String> = (0..10).map(|n| format!("90000000{n:02}")).collect();
    let request = TimelineRequest {
        view: TimelineView::Threads,
        thread_id: None,
        before: None,
        after: None,
        limit: 50,
    };
    for channel in &channels {
        for thread in h.upstream.add(channel, MAX_QUEUED_PER_CHANNEL + 8) {
            let _never = h.upstream.hold(&thread);
        }
        let id = ChannelId(channel.clone());
        let mut page = h
            .counting
            .fetch_timeline(&id, &request)
            .await
            .expect("a list");
        names.annotate(&deps, &id, &request, &mut page, true).await;
    }
    // A millisecond: every worker runs, and no read times out.
    tokio::time::sleep(Duration::from_millis(1)).await;
    let queued: Vec<usize> = channels
        .iter()
        .map(|channel| names.queued(&ChannelId(channel.clone())))
        .collect();
    assert!(
        queued.iter().all(|&count| count <= MAX_QUEUED_PER_CHANNEL),
        "{queued:?}"
    );
    assert!(
        queued.iter().sum::<usize>() <= MAX_QUEUED,
        "{} wait in all: {queued:?}",
        queued.iter().sum::<usize>()
    );
    assert!(
        names.pending() >= MAX_QUEUED && names.pending() <= MAX_QUEUED + 1,
        "{} pending: the global cap, and at most the one being read",
        names.pending()
    );
    assert!(
        queued[8] + queued[9] <= 1,
        "the last channels did not find the queues full: {queued:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_thread_waits_at_most_the_queue_limit_and_a_later_read_queues_it_again() {
    // A budget of twenty minutes, so the third thread's turn would come at forty: past the limit,
    // it is dropped unread rather than kept waiting. The owner's next read queues it again.
    let interval = Duration::from_secs(20 * 60);
    assert!(2 * interval > MAX_QUEUED_WAIT);
    let h = harness_with(ThreadNames::with_interval(interval));
    let ids = h.upstream.add(WRITE_CHANNEL, 3);
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(1).await;
    assert_eq!(h.names.pending(), 2);
    h.at(45 * 60).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [ids[2].clone(), ids[1].clone()],
        "the thread past the queue limit was read anyway"
    );
    assert_eq!(h.names.pending(), 0, "a thread waited past the limit");

    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(45 * 60 + 1).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [ids[2].clone(), ids[1].clone(), ids[0].clone()]
    );
    assert_eq!(h.names.named(), 3);
}

#[tokio::test(start_paused = true)]
async fn taking_a_channel_off_the_list_drops_its_queue_and_files_nothing_in_flight() {
    let h = harness();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let ids = h.upstream.add(READ_CHANNEL, 5);
    let gate = h.upstream.hold(&ids[4]);
    h.list(READ_CHANNEL, WRITE_TOKEN).await;
    h.at(1).await;
    assert_eq!(h.named_reads(READ_CHANNEL), [ids[4].clone()]);
    assert_eq!(h.names.queued(&channel), 4);

    let (status, body) = h
        .send(
            "DELETE",
            &format!("/api/v1/channels/{READ_CHANNEL}"),
            WRITE_TOKEN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        h.names.queued(&channel),
        0,
        "the queue outlived its channel"
    );

    // The read under way answers after the channel went: nothing is asked or filed for it.
    gate.add_permits(1);
    h.at(2).await;
    assert_eq!(h.names.pending(), 0);
    assert_eq!(h.names.named(), 0, "a hidden channel's thread was named");
    assert!(h.summarizer.thread_prompts().is_empty());
    assert!(h
        .state
        .store
        .cached_summary(&name_key(&channel, &ids[4]))
        .await
        .expect("readable")
        .is_none());
    h.at(300).await;
    assert_eq!(
        h.thread_reads(READ_CHANNEL),
        1,
        "a hidden channel's queue was still read"
    );

    // Shown again, the owner's next read queues what is still due.
    let (status, body) = h
        .send(
            "DELETE",
            &format!("/api/v1/channels/{READ_CHANNEL}/hidden"),
            WRITE_TOKEN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.list(READ_CHANNEL, WRITE_TOKEN).await;
    h.at(301).await;
    assert_eq!(h.thread_reads(READ_CHANNEL), 2);
    assert_eq!(h.names.named(), 1);
}
