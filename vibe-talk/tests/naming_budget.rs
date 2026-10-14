//! remote-ops-audit W5 (naming-budget): how often naming a thread reads the chat provider.
//!
//! A thread is named only from a read of it made at its turn. These tests are about WHEN and HOW
//! OFTEN those reads happen: at most one per channel per thirty seconds by default, a thread with
//! no name first and then newest activity, none kept waiting for ever by threads falling due
//! faster than that, none in a channel for ten minutes after a provider failure there, none from a
//! read token, and no queue that grows without bound, waits for ever, or outlives its channel; and
//! about the copy of a thread a forward delta carries once it is named.
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
    name_key, ThreadDeps, ThreadNames, ThreadRequest, CHANNEL_PAUSE,
    DEFAULT_NAMING_INTERVAL_SECONDS, MAX_QUEUED, MAX_QUEUED_PER_CHANNEL, MAX_QUEUED_TURNS,
    MAX_QUEUED_WAIT, NAMING_READ_TIMEOUT, OVERDUE_TURNS,
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
    /// Its last activity, in seconds after 08:00.
    active_second: u32,
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
    /// Threads whose reads are answered from an older copy, with the `as_of` a bridge sends when
    /// it serves a copy it could not bring up to date.
    behind: Mutex<HashMap<String, (FakeThread, String)>>,
    /// How many of its newest replies every thread read leaves out, count and messages both, with
    /// no `as_of`: a provider whose thread view lags its thread list.
    thread_view_lag: Mutex<usize>,
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
    let second = thread.active_second;
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
        updated_at: format!(
            "2026-10-13T{:02}:{:02}:{:02}Z",
            8 + second / 3_600,
            (second / 60) % 60,
            second % 60
        ),
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
                // A minute apart.
                active_second: 60 * u32::try_from(n).unwrap(),
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
                let behind = self.behind.lock().unwrap().get(&id).cloned();
                let (found, as_of) = match behind {
                    Some((copy, as_of)) => (Some(copy), Some(as_of)),
                    None => (found, None),
                };
                let lag = *self.thread_view_lag.lock().unwrap();
                let found = found.map(|mut thread| {
                    let kept = thread.replies.len().saturating_sub(lag);
                    thread.replies.truncate(kept);
                    thread
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
                    as_of,
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

    /// A forward delta of `channel`'s thread list, read with `token`.
    async fn delta(&self, channel: &str, after: &str, token: &str) -> Value {
        let (status, body) = self
            .send(
                "GET",
                &format!(
                    "/api/v1/channels/{channel}/timeline?view=threads&after={}",
                    urlencode(after)
                ),
                token,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["delta"].is_object(), "this is a delta: {body}");
        body
    }

    /// What naming needs, reading through the counting client.
    fn deps(&self) -> ThreadDeps {
        ThreadDeps {
            chat: self.counting.clone(),
            ..ThreadDeps::from_state(&self.state)
        }
    }

    /// Read `channel`'s thread list and annotate it as a page's read is, without the HTTP routes:
    /// for the long runs below, and for answers a test lands out of order.
    async fn poll(&self, channel: &str, may_write: bool) -> TimelinePage {
        let id = ChannelId(channel.to_owned());
        let mut page = self
            .counting
            .fetch_timeline(&id, &list_request())
            .await
            .expect("a list");
        self.annotate(channel, &mut page, may_write).await;
        page
    }

    /// Annotate a thread-list answer that was read earlier, as if it landed now.
    async fn annotate(&self, channel: &str, page: &mut TimelinePage, may_write: bool) {
        self.names
            .annotate(
                &self.deps(),
                &ChannelId(channel.to_owned()),
                &list_request(),
                page,
                may_write,
            )
            .await;
    }

    /// What a read token's forward delta of `channel`'s thread list carries when upstream changed
    /// nothing: the threads renamed lately, as this server holds them.
    async fn carried(&self, channel: &str) -> Vec<ThreadSummary> {
        let request = TimelineRequest {
            after: Some("a forward cursor".to_owned()),
            ..list_request()
        };
        let mut page = TimelinePage {
            has_threads: true,
            ..TimelinePage::default()
        };
        self.names
            .annotate(
                &self.deps(),
                &ChannelId(channel.to_owned()),
                &request,
                &mut page,
                false,
            )
            .await;
        page.threads
    }
}

fn list_request() -> TimelineRequest {
    TimelineRequest {
        view: TimelineView::Threads,
        thread_id: None,
        before: None,
        after: None,
        limit: 50,
    }
}

fn find<'a>(body: &'a Value, id: &str) -> Option<&'a Value> {
    body["threads"]
        .as_array()
        .expect("threads")
        .iter()
        .find(|thread| thread["id"].as_str() == Some(id))
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
        thread.active_second = 30 * 60;
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
async fn at_a_long_interval_a_queued_thread_is_kept_until_its_turn() {
    // A budget of twenty minutes, so the third of three threads queued at once has its turn at
    // forty. Before, a thread was dropped unread after thirty minutes whatever the interval, and
    // the next read queued it again behind everything newer: at a long interval the limit came
    // before the turn. It now scales with the interval, and is never reached at the budget.
    let interval = Duration::from_secs(20 * 60);
    let h = harness_with(ThreadNames::with_interval(interval));
    let ids = h.upstream.add(WRITE_CHANNEL, 3);
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(1).await;
    assert_eq!(h.names.pending(), 2);
    h.at(40 * 60 + 1).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [ids[2].clone(), ids[1].clone(), ids[0].clone()],
        "a thread was dropped before its turn"
    );
    assert_eq!(h.names.pending(), 0);
    assert_eq!(h.names.named(), 3);
    assert_eq!(h.names.max_wait(), interval * MAX_QUEUED_TURNS);
}

#[tokio::test(start_paused = true)]
async fn a_thread_its_channel_could_not_read_in_time_is_dropped_and_a_later_read_queues_it_again() {
    // At the default budget the limit is thirty minutes, more than the order ever makes a thread
    // wait while the channel is read at its budget. A channel read more slowly can reach it: here
    // three provider failures in a row pause it for ten minutes each, and the fourth thread, queued
    // with the rest at the start, has waited thirty minutes when the third pause ends. It is dropped
    // unread, and the owner's next read queues it again.
    let h = harness();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    assert_eq!(h.names.max_wait(), MAX_QUEUED_WAIT);
    let pause = CHANNEL_PAUSE.as_secs();
    assert_eq!(3 * pause, MAX_QUEUED_WAIT.as_secs());
    let ids = h.upstream.add(WRITE_CHANNEL, 4);
    for _ in 0..3 {
        h.counting
            .fail_next("fetch_timeline", Some(TimelineView::Thread), bad_gateway());
    }
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(3 * pause - 1).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [ids[3].clone(), ids[2].clone(), ids[1].clone()],
        "three failed reads, ten minutes apart"
    );
    assert_eq!(h.names.queued(&channel), 1);
    h.at(3 * pause + 1).await;
    assert_eq!(h.names.pending(), 0, "a thread waited past the limit");
    assert_eq!(
        h.thread_reads(WRITE_CHANNEL),
        3,
        "the thread past the queue limit was read anyway"
    );

    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(3 * pause + 120).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL)[3..],
        [
            ids[3].clone(),
            ids[2].clone(),
            ids[1].clone(),
            ids[0].clone()
        ]
    );
    assert_eq!(h.names.named(), 4);
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

// --- the copy a forward delta carries -----------------------------------------------------------

/// Change `thread` as one more reply at `second` (after 08:00) does.
fn reply_at(h: &Harness, thread: &str, second: u32) {
    h.upstream.change(thread, |thread| {
        thread.replies.push(format!("a reply at {second}s"));
        thread.active_second = second;
    });
}

#[tokio::test(start_paused = true)]
async fn a_page_that_saw_a_thread_change_while_it_waited_is_not_given_the_older_copy_back() {
    // Three threads are queued at once, so the oldest waits sixty seconds for its turn. Five
    // seconds in it gets a third reply, and a read token's delta shows it. Once it is named, the
    // next delta carries it, named — and must not carry the copy the owner's page queued it with:
    // the page replaces its copy whole and re-sorts by last activity, so the reply count, the
    // last activity and the order would all go back.
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 3);
    let oldest = ids[0].clone();
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    let first = h.list(WRITE_CHANNEL, READ_TOKEN).await;
    let after = first["next_after"].as_str().expect("a cursor").to_owned();

    h.at(5).await;
    reply_at(&h, &oldest, 10 * 60);
    h.at(6).await;
    let delta = h.delta(WRITE_CHANNEL, &after, READ_TOKEN).await;
    let seen = find(&delta, &oldest).expect("the changed thread is on the delta");
    assert_eq!(seen["reply_count"], 3, "{seen}");
    let after = delta["next_after"].as_str().expect("a cursor").to_owned();

    h.at(95).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [ids[2].clone(), ids[1].clone(), oldest.clone()]
    );
    let delta = h.delta(WRITE_CHANNEL, &after, READ_TOKEN).await;
    let carried = find(&delta, &oldest).expect("the renamed thread rides the delta");
    assert!(carried["display_name"].is_string(), "{carried}");
    assert_eq!(
        carried["reply_count"], 3,
        "the delta took back a reply the page had shown: {carried}"
    );
    assert_eq!(
        carried["updated_at"], "2026-10-13T08:10:00Z",
        "the delta took back the thread's last activity: {carried}"
    );
}

#[tokio::test(start_paused = true)]
async fn an_older_answer_landing_after_a_newer_one_while_a_thread_waits_is_not_what_a_delta_carries(
) {
    // Answers can land out of order: two reads under way at once, a read sent again. `older`
    // waits behind a newer thread; a page showing it with a third reply lands, and then one read
    // before that reply, without it. Neither is named from, and neither may take the copy a delta
    // carries back, nor the message count its sentence is filed at.
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 2);
    let (older, newer) = (ids[0].clone(), ids[1].clone());
    let mut stale = h.poll(WRITE_CHANNEL, true).await;
    h.at(1).await;
    assert_eq!(h.named_reads(WRITE_CHANNEL), std::slice::from_ref(&newer));

    reply_at(&h, &older, 10 * 60);
    h.poll(WRITE_CHANNEL, true).await;
    h.annotate(WRITE_CHANNEL, &mut stale, true).await;

    h.at(40).await;
    assert_eq!(h.named_reads(WRITE_CHANNEL), [newer.clone(), older.clone()]);
    let carried = h.carried(WRITE_CHANNEL).await;
    let carried = carried
        .iter()
        .find(|thread| thread.id == older)
        .expect("the renamed thread rides the delta");
    assert!(carried.display_name.is_some());
    assert_eq!(
        (carried.reply_count, carried.updated_at.as_str()),
        (Some(3), "2026-10-13T08:10:00Z"),
        "an older answer reached the delta"
    );

    // The read at its turn held four messages, so the sentence is filed at four, and a page
    // showing those four does not make the thread due again.
    h.at(45).await;
    h.poll(WRITE_CHANNEL, true).await;
    h.at(100).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [newer, older],
        "the sentence was filed at fewer messages than its read held, and cost another read"
    );
}

#[tokio::test(start_paused = true)]
async fn a_read_at_the_turn_that_says_it_is_behind_does_not_take_back_a_newer_copy() {
    // A bridge answers from a copy it keeps, and says with `as_of` when it could not bring that
    // copy up to date. Such a read at a thread's turn can be older than what a page has already
    // shown. The name is still made from that read alone; the copy a delta carries is the newer.
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 2);
    let (older, newer) = (ids[0].clone(), ids[1].clone());
    let before = h.upstream.channels.lock().unwrap()[WRITE_CHANNEL][0].clone();
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(1).await;

    reply_at(&h, &older, 10 * 60);
    h.upstream
        .behind
        .lock()
        .unwrap()
        .insert(older.clone(), (before, "2026-10-13T08:05:00Z".to_owned()));
    let page = h.list(WRITE_CHANNEL, READ_TOKEN).await;
    assert_eq!(find(&page, &older).expect("listed")["reply_count"], 3);

    h.at(31).await;
    assert_eq!(h.named_reads(WRITE_CHANNEL), [newer.clone(), older.clone()]);
    let prompts = h.summarizer.thread_prompts();
    assert!(
        !prompts[1].contains("a reply at 600s"),
        "named from something other than the read at its turn: {}",
        prompts[1]
    );
    let carried = h.carried(WRITE_CHANNEL).await;
    let carried = carried
        .iter()
        .find(|thread| thread.id == older)
        .expect("the renamed thread rides the delta");
    assert_eq!(
        (carried.reply_count, carried.updated_at.as_str()),
        (Some(3), "2026-10-13T08:10:00Z"),
        "a read that said it was behind took back a newer copy"
    );
}

#[tokio::test(start_paused = true)]
async fn a_read_that_says_it_is_behind_does_not_bring_back_a_reply_deleted_while_waiting() {
    reply_deleted_while_waiting(false).await;
}

#[tokio::test(start_paused = true)]
async fn a_reply_deleted_without_moving_the_threads_last_activity_is_not_brought_back_either() {
    reply_deleted_while_waiting(true).await;
}

/// `older`, with three replies, waits behind a newer thread. Five seconds in, one of its replies is
/// deleted upstream — its last reply, so its last activity goes back too, or with
/// `same_activity` its first — and a bridge goes on answering reads of the thread from its copy
/// from before the deletion, saying so with `as_of`. A read token's page shows the deletion. The
/// name is made from the read at its turn, but the copy a delta carries is what that page showed:
/// fewer replies and no later activity look older than the read's copy, and are newer.
async fn reply_deleted_while_waiting(same_activity: bool) {
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 2);
    let (older, newer) = (ids[0].clone(), ids[1].clone());
    reply_at(&h, &older, 30);
    let before = h.upstream.channels.lock().unwrap()[WRITE_CHANNEL][0].clone();
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(1).await;
    assert_eq!(h.named_reads(WRITE_CHANNEL), std::slice::from_ref(&newer));

    h.at(5).await;
    h.upstream.change(&older, |thread| {
        if same_activity {
            thread.replies.remove(0);
        } else {
            thread.replies.pop();
            thread.active_second = 0;
        }
    });
    h.upstream
        .behind
        .lock()
        .unwrap()
        .insert(older.clone(), (before, "2026-10-13T08:05:00Z".to_owned()));
    h.at(6).await;
    let page = h.list(WRITE_CHANNEL, READ_TOKEN).await;
    let shown = find(&page, &older).expect("listed").clone();
    assert_eq!(shown["reply_count"], 2, "{shown}");
    let after = page["next_after"].as_str().expect("a cursor").to_owned();

    h.at(40).await;
    assert_eq!(h.named_reads(WRITE_CHANNEL), [newer, older.clone()]);
    let prompts = h.summarizer.thread_prompts();
    assert!(
        prompts[1].contains("a reply at 30s"),
        "named from something other than the read at its turn: {}",
        prompts[1]
    );
    let delta = h.delta(WRITE_CHANNEL, &after, READ_TOKEN).await;
    let carried = find(&delta, &older).expect("the renamed thread rides the delta");
    assert!(carried["display_name"].is_string(), "{carried}");
    assert_eq!(
        (&carried["reply_count"], &carried["updated_at"]),
        (&shown["reply_count"], &shown["updated_at"]),
        "the delta brought back a deleted reply: {carried}"
    );
}

// --- a read that is behind ----------------------------------------------------------------------

/// How many times each of `threads` was read to name it, in order.
fn reads_of(h: &Harness, threads: &[String]) -> Vec<usize> {
    let reads = h.named_reads(WRITE_CHANNEL);
    threads
        .iter()
        .map(|thread| reads.iter().filter(|read| *read == thread).count())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_read_at_the_turn_that_says_it_is_behind_is_not_made_again_at_every_turn() {
    // As in `a_read_at_the_turn_that_says_it_is_behind_does_not_take_back_a_newer_copy`: `older`
    // gets a third reply while it waits, a page shows it, and the read at its turn is answered
    // from the copy before that reply, with `as_of`. The bridge stays behind for ten minutes while
    // a write-scope page polls the list every five seconds. The sentence must be filed at no fewer
    // messages than the page showed: filed at the read's count, the next poll finds the thread
    // due, and the read at every later turn answers the same.
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 2);
    let (older, newer) = (ids[0].clone(), ids[1].clone());
    let before = h.upstream.channels.lock().unwrap()[WRITE_CHANNEL][0].clone();
    h.list(WRITE_CHANNEL, WRITE_TOKEN).await;
    h.at(1).await;

    reply_at(&h, &older, 10 * 60);
    h.upstream
        .behind
        .lock()
        .unwrap()
        .insert(older.clone(), (before, "2026-10-13T08:05:00Z".to_owned()));
    let page = h.list(WRITE_CHANNEL, READ_TOKEN).await;
    assert_eq!(find(&page, &older).expect("listed")["reply_count"], 3);

    paced(&h, (5, 605, 5), |_, _| {}).await;
    assert_eq!(
        reads_of(&h, &[older, newer]),
        [1, 1],
        "a read that says it is behind was made again at later turns"
    );
    assert_eq!(h.summarizer.thread_prompts().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_named_thread_whose_fresh_read_says_it_is_behind_is_read_once() {
    // A thread is named with two replies. Ten seconds in it gets a third, which makes it due a
    // fresh sentence, and from then on the bridge answers reads of it from the copy before that
    // reply, with `as_of`, for ten minutes, while a write-scope page polls the list every five
    // seconds. Before W5 this cost one read; it must not cost one every interval.
    let h = harness();
    let thread = h.upstream.add(WRITE_CHANNEL, 1).remove(0);
    h.poll(WRITE_CHANNEL, true).await;
    h.at(1).await;
    assert_eq!(reads_of(&h, std::slice::from_ref(&thread)), [1]);
    let before = h.upstream.channels.lock().unwrap()[WRITE_CHANNEL][0].clone();

    h.at(10).await;
    reply_at(&h, &thread, 10 * 60);
    h.upstream
        .behind
        .lock()
        .unwrap()
        .insert(thread.clone(), (before, "2026-10-13T08:00:00Z".to_owned()));
    paced(&h, (10, 610, 5), |_, _| {}).await;
    assert_eq!(
        reads_of(&h, std::slice::from_ref(&thread)),
        [2],
        "the fresh read that said it was behind was made again at later turns"
    );
    assert_eq!(h.summarizer.thread_prompts().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_provider_whose_thread_view_counts_a_reply_fewer_than_its_list_names_each_thread_once() {
    // No `as_of` here: the thread view simply lags the list by one reply, every time. Each thread
    // is named once in half an hour of write-scope polls every three seconds. Filed at the read's
    // count, a thread with 1, 3 or 7 replies on the list was due again at the next poll, for ever.
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 5);
    for (thread, replies) in ids.iter().zip([1, 3, 7, 2, 10]) {
        h.upstream.change(thread, |thread| {
            thread.replies = (0..replies).map(|n| format!("reply {n}")).collect();
        });
    }
    *h.upstream.thread_view_lag.lock().unwrap() = 1;

    paced(&h, (0, 1_800, 3), |_, _| {}).await;
    assert_eq!(
        reads_of(&h, &ids),
        [1, 1, 1, 1, 1],
        "a thread whose read counted fewer replies than the list was read again"
    );
    assert_eq!(h.summarizer.thread_prompts().len(), ids.len());
}

/// A reply deleted at the start, where `named_at` is `None`, or, with `Some(replies)`, after the
/// thread was named with two replies and grew to `replies`: the list shows the deletion, and the
/// read at the thread's turn is answered from the bridge's copy from before it, with `as_of`, so
/// the sentence is made from the deleted reply — the read at its turn is all a name is made from.
/// The bridge then catches up, one new reply arrives, and a write-scope page polls the list every
/// five seconds for over ten minutes. Returns how many times the thread was read in all, and the
/// prompt of its last naming.
async fn deleted_while_behind_then_a_reply(named_at: Option<usize>) -> (usize, String) {
    let h = harness();
    let thread = h.upstream.add(WRITE_CHANNEL, 1).remove(0);
    let replies = match named_at {
        None => 1,
        Some(replies) => {
            h.poll(WRITE_CHANNEL, true).await;
            h.at(5).await;
            assert_eq!(reads_of(&h, std::slice::from_ref(&thread)), [1]);
            h.at(40).await;
            replies
        }
    };
    h.upstream.change(&thread, |thread| {
        thread.replies = (1..replies).map(|n| format!("reply {n}")).collect();
        thread.replies.push("a reply deleted since".to_owned());
        thread.active_second = 40;
    });
    let before = h.upstream.channels.lock().unwrap()[WRITE_CHANNEL][0].clone();
    h.upstream.change(&thread, |thread| {
        thread.replies.pop();
        thread.active_second = 35;
    });
    h.upstream
        .behind
        .lock()
        .unwrap()
        .insert(thread.clone(), (before, "2026-10-13T08:00:40Z".to_owned()));
    let shown = h.poll(WRITE_CHANNEL, true).await;
    assert_eq!(shown.threads[0].reply_count, Some(replies as u64 - 1));
    let at = named_at.map_or(0, |_| 40);
    h.at(at + 30).await;
    let prompts = h.summarizer.thread_prompts();
    assert_eq!(prompts.len(), if named_at.is_some() { 2 } else { 1 });
    assert!(
        prompts.last().unwrap().contains("a reply deleted since"),
        "named from something other than the read at its turn: {}",
        prompts.last().unwrap()
    );

    h.at(at + 60).await;
    h.upstream.behind.lock().unwrap().clear();
    h.upstream.change(&thread, |thread| {
        thread.replies.push("a reply after the deletion".to_owned());
        thread.active_second = u32::try_from(at + 60).unwrap();
    });
    paced(&h, (at + 60, at + 700, 5), |_, _| {}).await;
    let reads = reads_of(&h, std::slice::from_ref(&thread))[0];
    let last = h.summarizer.thread_prompts().pop().unwrap();
    // What a page serves is the sentence of that last naming.
    let page = h.poll(WRITE_CHANNEL, false).await;
    assert_eq!(
        page.threads[0].summary.as_deref(),
        Some(format!("thread summary #{}", h.summarizer.thread_prompts().len()).as_str())
    );
    (reads, last)
}

#[tokio::test(start_paused = true)]
async fn a_reply_deleted_while_the_read_at_the_turn_was_behind_leaves_the_sentence_at_the_next_reply(
) {
    // An unnamed thread whose only reply was deleted: the list shows none, and the read at its
    // turn still holds it. Filed at that read's count, two messages, the sentence was not due again
    // when the next reply brought the thread to two, and the deleted reply's text was served for as
    // long as the thread stayed there. Before W5, and now, it is filed at the one message the list
    // showed, and the next reply makes it due.
    let (reads, last) = deleted_while_behind_then_a_reply(None).await;
    assert_eq!(
        reads, 2,
        "the reply after the deletion did not make the thread due"
    );
    assert!(
        last.contains("a reply after the deletion") && !last.contains("a reply deleted since"),
        "the sentence still holds the deleted reply: {last}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_reply_deleted_while_a_fresh_read_was_behind_leaves_the_sentence_at_the_next_reply() {
    // The same further up the schedule: named at three messages, the thread grows to seven
    // replies, the last is deleted (the list shows six, seven messages, which is due), and the read
    // is from before the deletion. Filed at that read's eight messages, the next fresh sentence was
    // not due until eighteen; filed at the list's seven, the next reply makes it due at eight.
    let (reads, last) = deleted_while_behind_then_a_reply(Some(7)).await;
    assert_eq!(
        reads, 3,
        "the reply after the deletion did not make the thread due"
    );
    assert!(
        last.contains("a reply after the deletion") && !last.contains("a reply deleted since"),
        "the sentence still holds the deleted reply: {last}"
    );
}

// --- the order threads are read in --------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_waiting_thread_a_later_page_shows_active_again_is_read_by_that_activity() {
    // The queue is read newest activity first, and a thread's activity is the newest a write-scope
    // page has shown, not the one it was first queued with.
    let h = harness();
    let ids = h.upstream.add(WRITE_CHANNEL, 3);
    h.poll(WRITE_CHANNEL, true).await;
    h.at(1).await;
    assert_eq!(h.named_reads(WRITE_CHANNEL), [ids[2].clone()]);

    h.at(10).await;
    reply_at(&h, &ids[0], 30 * 60);
    h.poll(WRITE_CHANNEL, true).await;
    h.at(31).await;
    assert_eq!(
        h.named_reads(WRITE_CHANNEL),
        [ids[2].clone(), ids[0].clone()],
        "the thread with the newest activity was not read next"
    );
}

/// A paced run: from second `from` to second `to`, every `step` seconds, `each_step` changes
/// upstream and a write-scope page then polls the list. Returns, per thread, the seconds at which
/// it was read in that time, each counted at the first poll that saw it.
async fn paced(
    h: &Harness,
    (from, to, step): (u64, u64, u64),
    mut each_step: impl FnMut(&Harness, u64),
) -> HashMap<String, Vec<u64>> {
    let mut reads: HashMap<String, Vec<u64>> = HashMap::new();
    let mut seen = h.named_reads(WRITE_CHANNEL).len();
    for second in (from..=to).step_by(usize::try_from(step).unwrap()) {
        h.at(second).await;
        each_step(h, second);
        h.poll(WRITE_CHANNEL, true).await;
        tokio::task::yield_now().await;
        let all = h.named_reads(WRITE_CHANNEL);
        for thread in &all[seen..] {
            reads.entry(thread.clone()).or_default().push(second);
        }
        seen = all.len();
    }
    reads
}

/// The paced run behind the next tests: from second `from` for `minutes`, `hot` threads each get a
/// reply every `every` seconds, `each_second` changes anything else, and a write-scope page polls
/// the list every second. Returns, per thread, the seconds at which it was read in that time.
async fn under_load(
    h: &Harness,
    hot: &[String],
    every: u64,
    (from, minutes): (u64, u64),
    mut each_second: impl FnMut(&Harness, u64),
) -> HashMap<String, Vec<u64>> {
    paced(h, (from, from + minutes * 60, 1), |h, second| {
        if second > 0 && second % every == 0 {
            for thread in hot {
                reply_at(h, thread, 3_600 + u32::try_from(second).unwrap());
            }
        }
        each_second(h, second);
    })
    .await
}

#[tokio::test(start_paused = true)]
async fn an_old_unnamed_thread_is_read_within_a_bound_while_hot_threads_stay_due() {
    // More due threads than the budget reads, for ever: hot threads replied to every few seconds,
    // each due a fresh sentence again within a minute or so. Before, the old thread was never read
    // in an hour. A thread with no name at all is read ahead of any that has one and is due only
    // a fresh sentence, so the only threads read before it are the hot ones queued with it, which
    // have no name yet either and newer activity: it is read at the turn after theirs.
    for (hot, every) in [(2, 5), (2, 6), (3, 5), (4, 12), (6, 5)] {
        let h = harness();
        let old = h.upstream.add(WRITE_CHANNEL, 1).remove(0);
        let hot_ids = h.upstream.add(WRITE_CHANNEL, hot);
        let reads = under_load(&h, &hot_ids, every, (0, 20), |_, _| {}).await;
        let first = reads.get(&old).and_then(|at| at.first().copied());
        let bound = hot as u64 * DEFAULT_NAMING_INTERVAL_SECONDS;
        assert!(
            first.is_some_and(|at| at <= bound),
            "{hot} threads replied to every {every}s: the old thread was first read at {first:?}, \
             not within {bound}s"
        );
        assert_eq!(reads[&old].len(), 1, "it is not due again");
        for thread in &hot_ids {
            assert!(
                reads.get(thread).is_some_and(|at| at.len() >= 4),
                "{hot} threads replied to every {every}s: {thread} was read {:?}",
                reads.get(thread)
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn threads_equally_active_each_get_their_turn_when_they_fall_due_faster_than_the_budget() {
    // Three threads replied to every five seconds fall due a fresh sentence about every fifty
    // seconds each: more than the two reads a minute the channel is allowed. None is left out,
    // and none waits more than the overdue limit and one more turn between falling due and being
    // read, plus the time it takes to fall due again.
    let h = harness();
    let hot_ids = h.upstream.add(WRITE_CHANNEL, 3);
    let reads = under_load(&h, &hot_ids, 5, (0, 60), |_, _| {}).await;
    let counts: Vec<usize> = hot_ids
        .iter()
        .map(|thread| reads.get(thread).map_or(0, Vec::len))
        .collect();
    let total: usize = counts.iter().sum();
    assert!(total >= 115, "the budget went unused: {counts:?}");
    for (thread, count) in hot_ids.iter().zip(&counts) {
        assert!(
            *count >= total / 3 - 3,
            "{thread} had {count} of {total} reads: {counts:?}"
        );
        let at = &reads[thread];
        let longest = at
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .max()
            .unwrap_or(0);
        assert!(
            longest <= 4 * 60,
            "{thread} went {longest}s between two reads: {at:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_thread_due_only_a_fresh_sentence_is_read_within_the_overdue_limit_behind_new_threads() {
    // A new unnamed thread every ten seconds, three times what the channel's budget reads: each
    // goes ahead of a thread that already has a name. That thread, due a fresh sentence, is still
    // read once it has waited OVERDUE_TURNS intervals, at the next turn.
    let h = harness();
    let named = h.upstream.add(WRITE_CHANNEL, 1).remove(0);
    h.poll(WRITE_CHANNEL, true).await;
    h.at(1).await;
    assert_eq!(h.named_reads(WRITE_CHANNEL), std::slice::from_ref(&named));

    // From 30 seconds a new thread every ten seconds, each newer than every thread before it; the
    // first is read at once. At 31 seconds two more replies, four messages after three: due a fresh
    // sentence (1, 2, 4, 8...), and the earliest queued of all that wait from then on.
    let reads = under_load(&h, &[], 3_600, (1, 10), |h, second| {
        if second >= 30 && second % 10 == 0 {
            let new = h.upstream.add(WRITE_CHANNEL, 1).remove(0);
            h.upstream.change(&new, |thread| {
                thread.active_second = 3_600 + u32::try_from(second).unwrap();
            });
        }
        if second == 31 {
            reply_at(h, &named, 3_600 + 30);
            reply_at(h, &named, 3_600 + 31);
        }
    })
    .await;
    let queued_at = 31;
    let bound = queued_at + (OVERDUE_TURNS as u64 + 1) * DEFAULT_NAMING_INTERVAL_SECONDS;
    let again = reads.get(&named).and_then(|at| at.first().copied());
    assert!(
        again.is_some_and(|at| at <= bound),
        "the named thread due a fresh sentence was read again at {again:?}, not by {bound}s"
    );
    assert!(
        reads.len() >= 15,
        "the new threads were read too: {} threads",
        reads.len()
    );
}

/// Intervals from five minutes to the longest the configuration accepts, each with the step a
/// write-scope page polls at in the runs below.
const LONG_INTERVALS: [(u64, u64); 5] = [(300, 5), (450, 5), (600, 10), (1_800, 20), (3_600, 20)];

/// At a channel interval of `interval` seconds, one thread is named at 0 seconds and the first of
/// a new unnamed thread every third of an interval at `interval`, each newer than every thread
/// before it: three times what the budget reads. One poll later a thread falls due that every new
/// one goes ahead of until it is overdue — with `named`, the first thread, due a fresh sentence;
/// otherwise an old thread with no name yet, listed then. Returns the second it falls due, and the
/// seconds it was read in twelve intervals.
async fn behind_new_threads(interval: u64, poll: u64, named: bool) -> (u64, Vec<u64>) {
    let h = harness_with(ThreadNames::with_interval(Duration::from_secs(interval)));
    let first = h.upstream.add(WRITE_CHANNEL, 1).remove(0);
    h.poll(WRITE_CHANNEL, true).await;
    h.at(1).await;
    assert_eq!(h.named_reads(WRITE_CHANNEL), std::slice::from_ref(&first));
    let falls_due = interval + poll;
    let mut waiting = first.clone();
    let reads = paced(&h, (interval, 12 * interval, poll), |h, second| {
        let at = 3_600 + u32::try_from(second).unwrap();
        if (second - interval) % (interval / 3) == 0 {
            let new = h.upstream.add(WRITE_CHANNEL, 1).remove(0);
            h.upstream.change(&new, |thread| thread.active_second = at);
        }
        if second == falls_due {
            if named {
                // Four messages after three: due a fresh sentence (1, 2, 4, 8...).
                reply_at(h, &first, at - 1);
                reply_at(h, &first, at);
            } else {
                waiting = h.upstream.add(WRITE_CHANNEL, 1).remove(0);
                h.upstream
                    .change(&waiting, |thread| thread.active_second = 1);
            }
        }
    })
    .await;
    (falls_due, reads.get(&waiting).cloned().unwrap_or_default())
}

#[tokio::test(start_paused = true)]
async fn at_every_interval_a_thread_due_a_fresh_sentence_is_read_within_the_overdue_limit() {
    // Before, from 450 seconds on the thirty-minute wait limit came no later than OVERDUE_TURNS
    // intervals: the named thread was dropped and queued afresh before it could be overdue, and
    // was not read again in twelve intervals. Every read went to the newest thread.
    for (interval, poll) in LONG_INTERVALS {
        let (falls_due, reads) = behind_new_threads(interval, poll, true).await;
        let bound = falls_due + (u64::from(OVERDUE_TURNS) + 1) * interval;
        assert!(
            reads.first().is_some_and(|&at| at <= bound),
            "at {interval}s the named thread due a fresh sentence at {falls_due}s was read again \
             at {reads:?}, not by {bound}s"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn at_every_interval_an_old_unnamed_thread_is_read_within_the_overdue_limit() {
    // Before, at 600 seconds and longer the old thread was never read in hours.
    for (interval, poll) in LONG_INTERVALS {
        let (falls_due, reads) = behind_new_threads(interval, poll, false).await;
        let bound = falls_due + (u64::from(OVERDUE_TURNS) + 1) * interval;
        assert!(
            reads.first().is_some_and(|&at| at <= bound),
            "at {interval}s the old unnamed thread listed at {falls_due}s was read at {reads:?}, \
             not by {bound}s"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn at_a_long_interval_hot_threads_share_the_reads_and_a_quiet_one_due_a_sentence_still_gets_one(
) {
    // Two named threads replied to all the time, and one old named thread that falls due a fresh
    // sentence and then goes quiet. Before, at 600 seconds the old thread was never read in four
    // hours, and at 3600 in twelve, and at 3600 one hot thread had every read: the thirty-minute
    // limit dropped both hot threads before each turn, the next poll queued them again in page
    // order, and the tie went to the same one each time.
    for interval in [600, 3_600] {
        let h = harness_with(ThreadNames::with_interval(Duration::from_secs(interval)));
        let old = h.upstream.add(WRITE_CHANNEL, 1).remove(0);
        let hot = h.upstream.add(WRITE_CHANNEL, 2);
        let step = interval / 60;
        let named = paced(&h, (0, 2 * interval + step, step), |_, _| {}).await;
        assert_eq!(named.len(), 3, "each named once, one interval apart");

        let falls_due = 2 * interval + 2 * step;
        let turns = 10;
        let reads = paced(
            &h,
            (falls_due, falls_due + turns * interval, step),
            |h, second| {
                if second == falls_due {
                    // Seven messages after three, at the thread's old activity.
                    for _ in 0..4 {
                        h.upstream
                            .change(&old, |thread| thread.replies.push("one more".to_owned()));
                    }
                }
                for thread in &hot {
                    reply_at(h, thread, 3_600 + u32::try_from(second).unwrap());
                }
            },
        )
        .await;
        let bound = falls_due + (u64::from(OVERDUE_TURNS) + 1) * interval + step;
        let again = reads.get(&old).cloned().unwrap_or_default();
        assert!(
            again.first().is_some_and(|&at| at <= bound),
            "at {interval}s the old thread due a fresh sentence at {falls_due}s was read at \
             {again:?}, not by {bound}s"
        );
        let counts: Vec<usize> = hot
            .iter()
            .map(|thread| reads.get(thread).map_or(0, Vec::len))
            .collect();
        assert!(
            counts.iter().all(|&count| count >= 4),
            "at {interval}s the two hot threads had {counts:?} of {turns} reads"
        );
    }
}
