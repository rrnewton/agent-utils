//! One upstream read for identical timeline requests, and one window fetch for concurrent window
//! reads of a channel. `#278 timeline-single-flight`, remote-ops audit W4.
//!
//! Every test here COUNTS the calls that reach the provider for one thing the reader did, with
//! `testing::CountingChat` in front of the provider: the audit's measure of an O(requesters) path.
//! The provider answers after half a second, a fast answer from the slow bridge the audit measured,
//! so requests really do overlap. The clock is paused, so none of that time is spent.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::testing::{self, CountingChat, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::threads::{
    MessageThread, ThreadSummary, TimelinePage, TimelineRequest, TimelineView,
};
use vibe_talk::upstream_profile::Outcome;

const THREAD: &str = "thread-A";
/// How long the provider takes to answer.
const READ: Duration = Duration::from_millis(500);

fn message(id: u64, content: &str, thread: bool) -> Message {
    Message {
        id: MessageId(id.to_string()),
        channel_id: ChannelId(WRITE_CHANNEL.to_owned()),
        author: "Reader".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: format!("2026-10-13T10:{:02}:00Z", id % 60),
        spoken_time: String::new(),
        reply_to: None,
        content: content.to_owned(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        thread: thread.then(|| MessageThread {
            id: THREAD.to_owned(),
            root_message_id: Some(MessageId("10".to_owned())),
            is_root: id == 10,
            reply_count: Some(1),
            reply_count_exact: true,
        }),
        reactions: None,
        heading: None,
    }
}

/// A threaded provider whose channel holds a root, its reply, and whatever is posted.
struct Threaded {
    messages: Mutex<Vec<Message>>,
    /// When set, every read describes the channel as it stood when the read was ASKED and answers
    /// this long after, as a bridge's scan does, while a post lands at once. Unset, it answers at
    /// once (`CountingChat` adds the delay).
    scan: Option<Duration>,
}

impl Threaded {
    fn new() -> Self {
        Self {
            messages: Mutex::new(vec![message(10, "root", true), message(11, "reply", true)]),
            scan: None,
        }
    }

    /// A provider whose slow reads answer with the channel as it stood when they were asked.
    fn scanning(scan: Duration) -> Self {
        Self {
            scan: Some(scan),
            ..Self::new()
        }
    }

    /// The channel now, handed over `scan` later.
    async fn snapshot(&self) -> Vec<Message> {
        let snapshot = self.messages.lock().unwrap().clone();
        if let Some(scan) = self.scan {
            tokio::time::sleep(scan).await;
        }
        snapshot
    }

    fn summary(&self) -> ThreadSummary {
        ThreadSummary {
            id: THREAD.to_owned(),
            root: Some(message(10, "root", true)),
            title: "root".to_owned(),
            reply_count: Some(1),
            reply_count_exact: true,
            updated_at: "2026-10-13T10:11:00Z".to_owned(),
            display_name: None,
            summary: None,
        }
    }
}

#[async_trait::async_trait]
impl ChatClient for Threaded {
    fn provider_name(&self) -> &str {
        "Bridge"
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
        Ok(self.snapshot().await)
    }

    async fn fetch_timeline(
        &self,
        _channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        let threads = request.view == TimelineView::Threads;
        let snapshot = self.snapshot().await;
        Ok(TimelinePage {
            messages: if threads { Vec::new() } else { snapshot },
            threads: if threads {
                vec![self.summary()]
            } else {
                Vec::new()
            },
            thread: (request.view == TimelineView::Thread).then(|| self.summary()),
            has_threads: true,
            has_more: request.before.is_none(),
            next_before: request.before.is_none().then(|| "older".to_owned()),
            ..TimelinePage::default()
        })
    }

    async fn post_message(
        &self,
        _channel: &ChannelId,
        content: &str,
        _reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        let mut messages = self.messages.lock().unwrap();
        let posted = message(20 + messages.len() as u64, content, false);
        messages.push(posted.clone());
        Ok(posted)
    }
}

struct Harness {
    app: axum::Router,
    state: vibe_talk::state::AppState,
    chat: Arc<CountingChat>,
    /// The provider behind the counter, for a change made upstream behind this server's back.
    upstream: Arc<Threaded>,
    summarizer: Arc<vibe_talk::summarize::fake::FakeSummarizer>,
}

fn harness_with(toml: &str) -> Harness {
    harness_over(toml, Arc::new(Threaded::new()), READ)
}

/// Reads that take [`READ`] and describe the channel as it stood when they were asked, and posts
/// that land at once: the slow bridge scan, against which a read that began before a post can be
/// told from one that began after it.
fn scanning_harness() -> Harness {
    harness_over(
        &testing::config_toml(),
        Arc::new(Threaded::scanning(READ)),
        Duration::ZERO,
    )
}

/// `upstream` behind a counter that makes each call wait `delay`.
fn harness_over(toml: &str, upstream: Arc<Threaded>, delay: Duration) -> Harness {
    let summarizer = Arc::new(vibe_talk::summarize::fake::FakeSummarizer::naming_threads());
    let (mut state, _discord, _store) = testing::state_with_backend(toml, summarizer.clone());
    let chat = Arc::new(CountingChat::new(upstream.clone()).with_delay(delay));
    state.replace_chat(chat.clone());
    Harness {
        app: vibe_talk::http::router(state.clone()),
        state,
        chat,
        upstream,
        summarizer,
    }
}

fn harness() -> Harness {
    harness_with(&testing::config_toml())
}

/// The test configuration with `[chat]` set as given.
fn with_chat(settings: &str) -> String {
    testing::config_toml().replace("[discord]", &format!("[chat]\n{settings}\n\n[discord]"))
}

async fn call(
    app: axum::Router,
    method: &'static str,
    path: String,
    token: &'static str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(app: &axum::Router, path: &str, token: &'static str) -> (StatusCode, Value) {
    call(app.clone(), "GET", path.to_owned(), token, None).await
}

fn timeline(query: &str) -> String {
    format!("/api/v1/channels/{WRITE_CHANNEL}/timeline?{query}")
}

fn ids(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|m| m["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn five_identical_requests_at_once_cost_one_upstream_read() {
    let h = harness();
    let path = timeline("view=main");
    let answers =
        futures_util::future::join_all((0..5).map(|_| get(&h.app, &path, READ_TOKEN))).await;
    for (status, body) in &answers {
        assert_eq!(*status, StatusCode::OK, "{body}");
        assert_eq!(ids(body), ["10", "11"], "{body}");
        // Each caller's answer is its own: stamped for it, with its own forward cursor.
        assert!(body["next_after"].is_string(), "{body}");
        assert!(!body["messages"][0]["spoken_time"]
            .as_str()
            .unwrap_or_default()
            .is_empty());
    }
    assert_eq!(
        h.chat.count("fetch_timeline"),
        1,
        "five identical requests at once each read upstream: {:?}",
        h.chat.calls()
    );
}

#[tokio::test(start_paused = true)]
async fn an_abandoned_request_does_not_cancel_its_read_and_the_resend_joins_it() {
    // The page gives up on a read after twenty seconds and asks again, while the bridge's read runs
    // on to its own bound. Dropping the first request must not throw that read away.
    let h = harness();
    let path = timeline("view=flat");
    let first = tokio::spawn(call(h.app.clone(), "GET", path.clone(), READ_TOKEN, None));
    tokio::time::sleep(Duration::from_millis(100)).await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (status, body) = get(&h.app, &path, READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), ["10", "11"]);
    assert_eq!(
        h.chat.count("fetch_timeline"),
        1,
        "the re-send read again: {:?}",
        h.chat.calls()
    );
    // The read ran to its end, so the profile says it succeeded rather than that it was abandoned.
    let report = h.state.providers.profile().report();
    let rows = &report.providers[0].operations;
    let timeline_rows: Vec<_> = rows
        .iter()
        .filter(|row| row.name == "fetch_timeline")
        .map(|row| (row.outcome, row.count))
        .collect();
    assert_eq!(timeline_rows, [(Outcome::Ok, 1)], "{rows:?}");
}

#[tokio::test(start_paused = true)]
async fn a_different_view_or_thread_is_a_different_read() {
    let h = harness();
    let main = timeline("view=main");
    let flat = timeline("view=flat");
    let thread = timeline(&format!("view=thread&thread_id={THREAD}"));
    let (a, b, c) = tokio::join!(
        get(&h.app, &main, READ_TOKEN),
        get(&h.app, &flat, READ_TOKEN),
        get(&h.app, &thread, READ_TOKEN),
    );
    for (status, body) in [a, b, c] {
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    assert_eq!(h.chat.count("fetch_timeline"), 3, "{:?}", h.chat.calls());
    for view in [TimelineView::Main, TimelineView::Flat, TimelineView::Thread] {
        assert_eq!(h.chat.count_view("fetch_timeline", view), 1, "{view:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_newest_page_is_reused_for_three_seconds_and_read_again_after() {
    let h = harness();
    let path = timeline("view=main");
    assert_eq!(get(&h.app, &path, READ_TOKEN).await.0, StatusCode::OK);
    // Two seconds after the first read landed: the same answer, with no read.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (status, body) = get(&h.app, &path, READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), ["10", "11"]);
    assert_eq!(h.chat.count("fetch_timeline"), 1, "{:?}", h.chat.calls());
    // Four seconds after it: read again.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(get(&h.app, &path, READ_TOKEN).await.0, StatusCode::OK);
    assert_eq!(h.chat.count("fetch_timeline"), 2, "{:?}", h.chat.calls());
}

#[tokio::test(start_paused = true)]
async fn a_step_back_is_shared_while_it_runs_and_never_kept() {
    let h = harness();
    let older = timeline("view=main&before=older");
    let (a, b) = tokio::join!(
        get(&h.app, &older, READ_TOKEN),
        get(&h.app, &older, READ_TOKEN)
    );
    assert_eq!(a.0, StatusCode::OK, "{}", a.1);
    assert_eq!(b.0, StatusCode::OK, "{}", b.1);
    assert_eq!(h.chat.count("fetch_timeline"), 1, "{:?}", h.chat.calls());
    assert!(
        a.1["next_after"].is_null(),
        "a step back carries no forward cursor"
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(get(&h.app, &older, READ_TOKEN).await.0, StatusCode::OK);
    assert_eq!(h.chat.count("fetch_timeline"), 2, "{:?}", h.chat.calls());
}

#[tokio::test(start_paused = true)]
async fn with_the_memo_off_only_a_read_in_flight_is_shared() {
    let h = harness_with(&with_chat("timeline_memo_ms = 0"));
    let path = timeline("view=main");
    let (a, b) = tokio::join!(
        get(&h.app, &path, READ_TOKEN),
        get(&h.app, &path, READ_TOKEN)
    );
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!(h.chat.count("fetch_timeline"), 1, "{:?}", h.chat.calls());
    assert_eq!(get(&h.app, &path, READ_TOKEN).await.0, StatusCode::OK);
    assert_eq!(h.chat.count("fetch_timeline"), 2, "{:?}", h.chat.calls());
}

#[tokio::test(start_paused = true)]
async fn a_change_this_server_hears_of_makes_the_next_read_fresh() {
    // The page re-reads after a live message, and after its own post. Neither may be answered by a
    // read from before the change, joined or remembered.
    let h = harness();
    let path = timeline("view=main");
    assert_eq!(get(&h.app, &path, READ_TOKEN).await.0, StatusCode::OK);
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    h.state.live.publish(&channel, message(12, "pushed", false));
    let (_, body) = get(&h.app, &path, READ_TOKEN).await;
    assert_eq!(
        h.chat.count("fetch_timeline"),
        2,
        "a push left the memo in place"
    );
    assert_eq!(
        ids(&body),
        ["10", "11"],
        "the provider's own answer, as it stands"
    );

    let (status, posted) = call(
        h.app.clone(),
        "POST",
        format!("/api/v1/channels/{WRITE_CHANNEL}/reply"),
        WRITE_TOKEN,
        Some(json!({ "text": "my own answer" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    let (_, body) = get(&h.app, &path, READ_TOKEN).await;
    assert_eq!(
        h.chat.count("fetch_timeline"),
        3,
        "a post left the memo in place"
    );
    assert!(
        ids(&body).contains(&posted["posted"]["id"].as_str().unwrap().to_owned()),
        "the read after a post did not show it: {body}"
    );

    // The storage purge empties the memory too: channel text the owner asked to go.
    let (status, purged) = call(
        h.app.clone(),
        "DELETE",
        "/api/v1/storage".to_owned(),
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{purged}");
    assert!(h.state.read_ahead.timelines().is_empty());
    assert_eq!(get(&h.app, &path, READ_TOKEN).await.0, StatusCode::OK);
    assert_eq!(
        h.chat.count("fetch_timeline"),
        4,
        "the purge left the memo in place"
    );
}

#[tokio::test(start_paused = true)]
async fn a_read_token_join_never_queues_naming_and_a_write_token_one_does() {
    // Stamping, noise, naming and read state are each caller's own, after the shared read: a read
    // token never spends the model, whoever's read it joined.
    let h = harness();
    let list = timeline("view=threads");
    let (a, b) = tokio::join!(
        get(&h.app, &list, READ_TOKEN),
        get(&h.app, &list, READ_TOKEN)
    );
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(
        h.chat.count_view("fetch_timeline", TimelineView::Threads),
        1
    );
    assert_eq!(h.state.thread_names.pending(), 0);
    assert_eq!(h.state.thread_names.named(), 0);
    assert_eq!(h.summarizer.calls(), 0);
    assert_eq!(
        h.chat.count_view("fetch_timeline", TimelineView::Thread),
        0,
        "a read token's join read a thread to name it"
    );

    // A write token joining a read token's read queues the naming for itself.
    let (a, b) = tokio::join!(get(&h.app, &list, READ_TOKEN), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        get(&h.app, &list, WRITE_TOKEN).await
    });
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!(
        h.chat.count_view("fetch_timeline", TimelineView::Threads),
        2
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(h.state.thread_names.named(), 1, "the write token's naming");
    assert_eq!(h.chat.count_view("fetch_timeline", TimelineView::Thread), 1);
}

#[tokio::test(start_paused = true)]
async fn concurrent_window_reads_of_one_channel_share_one_fetch() {
    // `ops::messages` goes through the channel's window turn: the digest, the to-do list, a lookup
    // by id and the messages route asked at once read the window once.
    let (mut state, discord) = testing::state();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    discord.seed(&channel, "Grace", "first");
    discord.seed(&channel, "Ada", "second");
    let chat = Arc::new(CountingChat::new(discord).with_delay(READ));
    state.replace_chat(chat.clone());
    let app = vibe_talk::http::router(state);
    let path = format!("/api/v1/channels/{WRITE_CHANNEL}/messages");
    let answers =
        futures_util::future::join_all((0..5).map(|_| get(&app, &path, READ_TOKEN))).await;
    for (status, body) in &answers {
        assert_eq!(*status, StatusCode::OK, "{body}");
        assert_eq!(body["messages"].as_array().map(Vec::len), Some(2), "{body}");
    }
    assert_eq!(
        chat.count("fetch_recent_dated"),
        1,
        "five window reads at once each fetched: {:?}",
        chat.calls()
    );
    // A narrower read joining a wider one is answered at its own width.
    let (wide_path, narrow_path) = (format!("{path}?limit=20"), format!("{path}?limit=1"));
    let (wide, narrow) = tokio::join!(get(&app, &wide_path, READ_TOKEN), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        get(&app, &narrow_path, READ_TOKEN).await
    });
    assert_eq!(wide.1["messages"].as_array().map(Vec::len), Some(2));
    assert_eq!(narrow.1["messages"].as_array().map(Vec::len), Some(1));
    assert_eq!(narrow.1["messages"][0]["content"], "second", "the newest");
    assert_eq!(narrow.1["complete"], false);
    assert_eq!(chat.count("fetch_recent_dated"), 2, "{:?}", chat.calls());
}

#[tokio::test(start_paused = true)]
async fn a_shared_read_is_counted_under_the_reason_of_the_request_that_started_it() {
    // `#278 timeline-single-flight`, remote-ops audit W0.3. The read runs in a task of its own; it
    // still counts as the poll that started it, and the other column's re-read on coming back to
    // the page, made while it runs with no change between, joins it and costs nothing. (A `live`
    // re-read never joins: it follows a push, and a push starts a new generation.)
    let h = harness();
    let (poll, visible) = (
        timeline("view=main&reason=poll"),
        timeline("view=main&reason=visible"),
    );
    let (a, b) = tokio::join!(get(&h.app, &poll, READ_TOKEN), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        get(&h.app, &visible, READ_TOKEN).await
    });
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!(h.chat.count("fetch_timeline"), 1, "{:?}", h.chat.calls());
    let report = h.state.providers.profile().report();
    let reasons: Vec<_> = report.providers[0]
        .reasons
        .iter()
        .map(|row| (row.name, row.reason.as_str(), row.count))
        .collect();
    assert_eq!(reasons, [("fetch_timeline", "poll", 1)]);
}

/// A read of `path` begun at 0 ms, this server's own post at 50 ms, and the same read again at
/// 100 ms, while the first is still running: what the second answered, and the id posted.
async fn own_post_during_a_read(h: &Harness, path: &str) -> (Vec<String>, String) {
    let first = tokio::spawn(call(
        h.app.clone(),
        "GET",
        path.to_owned(),
        READ_TOKEN,
        None,
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (status, posted) = call(
        h.app.clone(),
        "POST",
        format!("/api/v1/channels/{WRITE_CHANNEL}/reply"),
        WRITE_TOKEN,
        Some(json!({ "text": "mine" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    let id = posted["posted"]["id"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (status, body) = get(&h.app, path, READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, early) = first.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{early}");
    assert!(
        !ids(&early).contains(&id),
        "the fixture's first read saw a post made after it was asked: {early}"
    );
    (ids(&body), id)
}

#[tokio::test(start_paused = true)]
async fn a_window_read_after_this_servers_own_post_shows_the_post() {
    // Review of #278: the window turn let a read made after this server's own post reuse a fetch
    // that began before it, because a post is not a live push until the provider echoes it. The
    // MCP tools, the digest, a lookup by id and bankruptcy all read the window right after posting.
    let h = scanning_harness();
    let path = format!("/api/v1/channels/{WRITE_CHANNEL}/messages");
    let (answered, id) = own_post_during_a_read(&h, &path).await;
    assert!(
        answered.contains(&id),
        "the window read made after our own post did not show it: {answered:?} (posted {id})"
    );
    assert_eq!(
        h.chat.count("fetch_recent_dated"),
        2,
        "the read after the post must be its own: {:?}",
        h.chat.calls()
    );
}

#[tokio::test(start_paused = true)]
async fn a_timeline_read_after_this_servers_own_post_shows_the_post() {
    let h = scanning_harness();
    let (answered, id) = own_post_during_a_read(&h, &timeline("view=main")).await;
    assert!(
        answered.contains(&id),
        "the timeline read made after our own post did not show it: {answered:?} (posted {id})"
    );
    assert_eq!(
        h.chat.count("fetch_timeline"),
        2,
        "the read after the post must be its own: {:?}",
        h.chat.calls()
    );
}

#[tokio::test(start_paused = true)]
async fn window_reads_with_no_post_between_them_still_share_one_fetch() {
    // The control for the two above, against the same slow scan: no post, one fetch.
    let h = scanning_harness();
    let path = format!("/api/v1/channels/{WRITE_CHANNEL}/messages");
    let (a, b) = tokio::join!(get(&h.app, &path, READ_TOKEN), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        get(&h.app, &path, READ_TOKEN).await
    });
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!(ids(&a.1), ids(&b.1));
    assert_eq!(
        h.chat.count("fetch_recent_dated"),
        1,
        "{:?}",
        h.chat.calls()
    );
}

#[tokio::test(start_paused = true)]
async fn a_pull_or_an_enter_inside_the_memo_shows_an_unpushed_upstream_change() {
    // Review of #278: the memo answered an explicit refresh with the read before it, so a message
    // posted upstream and not yet pushed stayed hidden from pull to refresh. Pull and enter now
    // read afresh; the poll still takes the kept answer.
    let h = harness();
    let poll = timeline("view=main&reason=poll");
    assert_eq!(get(&h.app, &poll, READ_TOKEN).await.0, StatusCode::OK);
    h.upstream
        .messages
        .lock()
        .unwrap()
        .push(message(30, "from another client, not pushed", false));
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (status, body) = get(&h.app, &timeline("view=main&reason=pull"), READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        ids(&body).contains(&"30".to_owned()),
        "pull to refresh 1 s after a poll did not show an upstream message: {:?}",
        ids(&body)
    );
    assert_eq!(h.chat.count("fetch_timeline"), 2, "{:?}", h.chat.calls());

    h.upstream
        .messages
        .lock()
        .unwrap()
        .push(message(31, "another, not pushed", false));
    let (_, body) = get(&h.app, &timeline("view=main&reason=enter"), READ_TOKEN).await;
    assert!(ids(&body).contains(&"31".to_owned()), "{:?}", ids(&body));
    assert_eq!(h.chat.count("fetch_timeline"), 3, "{:?}", h.chat.calls());

    // A poll right after is answered from the enter's read, which is kept as any other.
    assert_eq!(get(&h.app, &poll, READ_TOKEN).await.0, StatusCode::OK);
    assert_eq!(h.chat.count("fetch_timeline"), 3, "{:?}", h.chat.calls());

    // Two columns entering the same channel together still make one read.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let enter = timeline("view=main&reason=enter");
    let (a, b) = tokio::join!(get(&h.app, &enter, READ_TOKEN), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        get(&h.app, &enter, READ_TOKEN).await
    });
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!(h.chat.count("fetch_timeline"), 4, "{:?}", h.chat.calls());
}
