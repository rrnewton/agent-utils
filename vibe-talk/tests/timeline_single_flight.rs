//! One upstream read for identical timeline requests, and what a window read of a channel may take
//! from another's fetch. `#278 timeline-single-flight`, remote-ops audit W4.
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
    /// How long after a post lands upstream its answer comes back: the window in which the request
    /// that sent it can be dropped although the provider has delivered it.
    post_answer: Duration,
    /// How much longer than any other a read of Main takes, after the channel is read.
    main_scan: Duration,
}

impl Threaded {
    fn new() -> Self {
        Self {
            messages: Mutex::new(vec![message(10, "root", true), message(11, "reply", true)]),
            scan: None,
            post_answer: Duration::ZERO,
            main_scan: Duration::ZERO,
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
        if request.view == TimelineView::Main {
            tokio::time::sleep(self.main_scan).await;
        }
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
        let posted = {
            let mut messages = self.messages.lock().unwrap();
            let posted = message(20 + messages.len() as u64, content, false);
            messages.push(posted.clone());
            posted
        };
        if !self.post_answer.is_zero() {
            tokio::time::sleep(self.post_answer).await;
        }
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

/// A read of `path` begun at 0 ms, a message posted upstream at 100 ms that nobody pushes, and a
/// read of `second` at 200 ms, while the first is still running: what the second answered, and
/// what the first did.
async fn unpushed_message_during_a_read(
    h: &Harness,
    path: &str,
    second: &str,
) -> (Vec<String>, Vec<String>) {
    let first = tokio::spawn(call(
        h.app.clone(),
        "GET",
        path.to_owned(),
        READ_TOKEN,
        None,
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.upstream
        .messages
        .lock()
        .unwrap()
        .push(message(30, "from another client, not pushed", false));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (status, body) = get(&h.app, second, READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, early) = first.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{early}");
    (ids(&body), ids(&early))
}

#[tokio::test(start_paused = true)]
async fn a_window_read_takes_no_fetch_that_began_before_it_was_asked() {
    // Round-3 review of #278: `ops::messages` waited behind a fetch that began before it and took
    // it, so a message posted upstream in between, and not pushed, was missing. The MCP read tools,
    // the digest, the to-do list, a lookup by id and bankruptcy go through it, and each read the
    // window for itself before #278. Now it takes only a fetch that began after it was asked, and
    // reads beside one already running.
    let h = scanning_harness();
    let path = format!("/api/v1/channels/{WRITE_CHANNEL}/messages");
    let (answered, early) = unpushed_message_during_a_read(&h, &path, &path).await;
    assert!(
        !early.contains(&"30".to_owned()),
        "fixture: the first read saw a message posted after it was asked: {early:?}"
    );
    assert!(
        answered.contains(&"30".to_owned()),
        "a window read 200 ms into a fetch begun before an upstream message missed it: {answered:?}"
    );
    assert_eq!(
        h.chat.count("fetch_recent_dated"),
        2,
        "{:?}",
        h.chat.calls()
    );
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
async fn a_timeline_read_with_no_post_between_still_joins_the_read_running() {
    // The control for the timeline test above, against the same slow scan: no post, one read. (A
    // window read made while a fetch runs is its own whether or not there was a post: see
    // `a_window_read_takes_no_fetch_that_began_before_it_was_asked`.)
    let h = scanning_harness();
    let path = timeline("view=main");
    let (a, b) = tokio::join!(get(&h.app, &path, READ_TOKEN), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        get(&h.app, &path, READ_TOKEN).await
    });
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!(ids(&a.1), ids(&b.1));
    assert_eq!(h.chat.count("fetch_timeline"), 1, "{:?}", h.chat.calls());
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

    // An opening 100 ms into another's read makes its own (see the next test); a poll 100 ms into
    // an opening's read joins it.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let enter = timeline("view=main&reason=enter");
    let (a, b) = tokio::join!(get(&h.app, &enter, READ_TOKEN), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        get(&h.app, &enter, READ_TOKEN).await
    });
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!(h.chat.count("fetch_timeline"), 5, "{:?}", h.chat.calls());
    tokio::time::sleep(Duration::from_secs(4)).await;
    let (a, b) = tokio::join!(get(&h.app, &enter, READ_TOKEN), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        get(&h.app, &poll, READ_TOKEN).await
    });
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!(h.chat.count("fetch_timeline"), 6, "{:?}", h.chat.calls());
}

#[tokio::test(start_paused = true)]
async fn a_pull_or_an_enter_joins_no_read_that_began_before_it() {
    // Round-3 review of #278: a pull or an opening joined a read already running, however long
    // before it began, so a message posted upstream after that read began, and not pushed, stayed
    // hidden from the pull. The page reaches this on its own: its twenty seconds end the poll, the
    // bridge's read runs on, and the reader pulls after "Refresh failed". Now a pull or an opening
    // always reads for itself.
    for reason in ["pull", "enter"] {
        let h = scanning_harness();
        let (answered, early) = unpushed_message_during_a_read(
            &h,
            &timeline("view=main&reason=poll"),
            &timeline(&format!("view=main&reason={reason}")),
        )
        .await;
        assert_eq!(
            early,
            ["10", "11"],
            "fixture: the poll saw the later message"
        );
        assert!(
            answered.contains(&"30".to_owned()),
            "a {reason} 200 ms into the poll's read missed a message posted upstream at 100 ms: \
             {answered:?}"
        );
        assert_eq!(
            h.chat.count("fetch_timeline"),
            2,
            "{reason}: {:?}",
            h.chat.calls()
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_kept_answer_is_not_handed_out_after_a_later_read_of_the_channel_has_landed() {
    // Round-3 review of #278: Main's poll was kept; the root was edited upstream and not pushed; the
    // thread on the right was opened, read afresh, and showed the edit; and then Main's re-read on
    // coming back to the page was answered from the kept poll, from before the edit. The page folds
    // each answer into one store by id, so the older copy went back over the newer on screen, until
    // the next full read of Main. A later read of the channel, any view, now retires a kept answer.
    let h = harness();
    assert_eq!(
        get(&h.app, &timeline("view=main&reason=poll"), READ_TOKEN)
            .await
            .0,
        StatusCode::OK
    );
    h.upstream.messages.lock().unwrap()[0].content = "root, edited".to_owned();
    let (status, thread) = get(
        &h.app,
        &timeline(&format!("view=thread&thread_id={THREAD}&reason=enter")),
        READ_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    assert_eq!(contents(&thread)[0], "root, edited", "fixture: {thread}");
    let (status, main) = get(&h.app, &timeline("view=main&reason=visible"), READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{main}");
    assert_eq!(
        contents(&main),
        ["root, edited", "reply"],
        "Main was answered with text older than the thread's read had already shown"
    );
    assert_eq!(h.chat.count("fetch_timeline"), 3, "{:?}", h.chat.calls());

    // The control: with no later read of the channel between, the kept answer still serves.
    let poll = timeline("view=main&reason=poll");
    assert_eq!(get(&h.app, &poll, READ_TOKEN).await.0, StatusCode::OK);
    assert_eq!(h.chat.count("fetch_timeline"), 3, "{:?}", h.chat.calls());
    // ...and a read of another channel retires nothing.
    let other = format!(
        "/api/v1/channels/{}/timeline?view=main&reason=poll",
        testing::READ_CHANNEL
    );
    assert_eq!(get(&h.app, &other, READ_TOKEN).await.0, StatusCode::OK);
    assert_eq!(get(&h.app, &poll, READ_TOKEN).await.0, StatusCode::OK);
    assert_eq!(h.chat.count("fetch_timeline"), 4, "{:?}", h.chat.calls());
}

#[tokio::test(start_paused = true)]
async fn a_joined_read_overtaken_by_a_later_read_of_the_channel_is_not_handed_out() {
    // The same for a read joined while it runs. Main's poll is slow. The root is edited upstream
    // and not pushed, and the thread on the right is opened: its read begins after the poll's and
    // lands first, with the edit. A re-read of Main made after the thread's read began, but before
    // it landed, joined the poll; it is not handed the poll's older text when that lands. One made
    // before the thread's read began is, as a read of its own would have been as old.
    let upstream = Threaded {
        main_scan: Duration::from_millis(900),
        ..Threaded::scanning(Duration::from_millis(200))
    };
    let h = harness_over(&testing::config_toml(), Arc::new(upstream), Duration::ZERO);
    let (poll_path, visible) = (
        timeline("view=main&reason=poll"),
        timeline("view=main&reason=visible"),
    );
    let (poll, early, thread, late) = tokio::join!(
        get(&h.app, &poll_path, READ_TOKEN),
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            get(&h.app, &visible, READ_TOKEN).await
        },
        async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            h.upstream.messages.lock().unwrap()[0].content = "root, edited".to_owned();
            let thread = timeline(&format!("view=thread&thread_id={THREAD}&reason=enter"));
            get(&h.app, &thread, READ_TOKEN).await
        },
        async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            get(&h.app, &visible, READ_TOKEN).await
        },
    );
    for (status, body) in [&poll, &early, &thread, &late] {
        assert_eq!(*status, StatusCode::OK, "{body}");
    }
    assert_eq!(
        contents(&poll.1)[0],
        "root",
        "fixture: the poll saw the edit"
    );
    assert_eq!(
        contents(&thread.1)[0],
        "root, edited",
        "fixture: {}",
        thread.1
    );
    assert_eq!(
        contents(&early.1)[0],
        "root",
        "a re-read made before the thread's read began did not join the poll"
    );
    assert_eq!(
        contents(&late.1)[0],
        "root, edited",
        "a re-read of Main made after the thread's read began was handed the poll's older text"
    );
    assert_eq!(
        h.chat.count_view("fetch_timeline", TimelineView::Main),
        2,
        "{:?}",
        h.chat.calls()
    );
}

/// `path` asked `after` from now, and how long it took from when it was asked.
async fn timed_get(app: &axum::Router, path: &str, after: Duration) -> (StatusCode, Duration) {
    tokio::time::sleep(after).await;
    let asked = tokio::time::Instant::now();
    let (status, body) = get(app, path, READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (status, asked.elapsed())
}

#[tokio::test(start_paused = true)]
async fn a_window_read_that_a_running_fetch_cannot_answer_does_not_wait_for_it() {
    // Review of #278: every window read of a channel took its turn for the whole fetch, so a read
    // the running fetch could not answer — wider than it, or made after a change it began before —
    // waited the whole fetch out and then fetched again: twice the wait, where every window read
    // used to run beside the others. A voice tool, given fifteen seconds, behind a summary's fetch
    // on a slow bridge, is the case that matters. Now such a read runs beside the fetch, and only a
    // read that fetch can answer waits for it.
    let h = harness();
    let messages = format!("/api/v1/channels/{WRITE_CHANNEL}/messages");
    let (narrow, wide) = (
        format!("{messages}?limit=1"),
        format!("{messages}?limit=20"),
    );

    // Wider than the fetch running: read beside it.
    let (_, (_, waited)) = tokio::join!(
        timed_get(&h.app, &narrow, Duration::ZERO),
        timed_get(&h.app, &wide, Duration::from_millis(50)),
    );
    assert_eq!(waited, READ, "a wider read waited for a narrower fetch");
    assert_eq!(
        h.chat.count("fetch_recent_dated"),
        2,
        "{:?}",
        h.chat.calls()
    );

    // Made after a push the running fetch began before: read beside it.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let (_, (_, waited)) = tokio::join!(timed_get(&h.app, &wide, Duration::ZERO), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.state.live.publish(&channel, message(12, "pushed", false));
        timed_get(&h.app, &wide, Duration::from_millis(10)).await
    });
    assert_eq!(
        waited, READ,
        "a read after a push waited for a fetch from before it"
    );
    assert_eq!(
        h.chat.count("fetch_recent_dated"),
        4,
        "{:?}",
        h.chat.calls()
    );

    // As wide or narrower, with no change between: since the round-3 review this reads beside the
    // fetch too, which began before it was asked (`a_window_read_takes_no_fetch_that_began_before_
    // it_was_asked`). Speech preparation still waits for such a fetch and takes it: see
    // `tests/read_ahead.rs`.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (_, (_, waited)) = tokio::join!(
        timed_get(&h.app, &wide, Duration::ZERO),
        timed_get(&h.app, &narrow, Duration::from_millis(50)),
    );
    assert_eq!(
        waited, READ,
        "a window read waited for a fetch begun before it was asked"
    );
    assert_eq!(
        h.chat.count("fetch_recent_dated"),
        6,
        "{:?}",
        h.chat.calls()
    );
}

fn contents(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// A read of `path` begun at 0 ms; this server's post at 50 ms, which lands upstream at once but
/// whose request is dropped at 100 ms, before the provider's answer comes back (the page giving up
/// on it, a client disconnect, sign-out, a cancelled MCP request); the same read again at 150 ms,
/// while the first is still running. What the second answered.
async fn cancelled_post_during_a_read(h: &Harness, path: &str) -> Vec<String> {
    let first = tokio::spawn(call(
        h.app.clone(),
        "GET",
        path.to_owned(),
        READ_TOKEN,
        None,
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let post = tokio::spawn(call(
        h.app.clone(),
        "POST",
        format!("/api/v1/channels/{WRITE_CHANNEL}/reply"),
        WRITE_TOKEN,
        Some(json!({ "text": "mine" })),
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        h.upstream
            .messages
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.content == "mine"),
        "fixture: the post had not landed upstream"
    );
    post.abort();
    assert!(
        post.await.unwrap_err().is_cancelled(),
        "fixture: the post's request was answered before it was dropped"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (status, body) = get(&h.app, path, READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, early) = first.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{early}");
    assert!(
        !contents(&early).contains(&"mine".to_owned()),
        "the fixture's first read saw a post made after it was asked: {early}"
    );
    contents(&body)
}

/// The slow scan of [`scanning_harness`], with a post whose answer comes back a second after it
/// lands.
fn harness_with_slow_post_answers() -> Harness {
    let upstream = Threaded {
        post_answer: Duration::from_secs(1),
        ..Threaded::scanning(READ)
    };
    harness_over(&testing::config_toml(), Arc::new(upstream), Duration::ZERO)
}

#[tokio::test(start_paused = true)]
async fn a_timeline_read_after_a_post_whose_request_was_dropped_shows_the_post() {
    // Review of #278: the post was counted only once the provider answered it, so a post whose
    // request was dropped while the provider had it — delivered, and the page saying "Delivery
    // unconfirmed, check history" — was never counted, and the read after it joined one from
    // before it. It is counted now however the send ends.
    let h = harness_with_slow_post_answers();
    let answered = cancelled_post_during_a_read(&h, &timeline("view=main")).await;
    assert!(
        answered.contains(&"mine".to_owned()),
        "a timeline read after a delivered post whose request was dropped did not show it: \
         {answered:?}"
    );
    assert_eq!(h.chat.count("fetch_timeline"), 2, "{:?}", h.chat.calls());
}

#[tokio::test(start_paused = true)]
async fn a_window_read_after_a_post_whose_request_was_dropped_shows_the_post() {
    let h = harness_with_slow_post_answers();
    let path = format!("/api/v1/channels/{WRITE_CHANNEL}/messages");
    let answered = cancelled_post_during_a_read(&h, &path).await;
    assert!(
        answered.contains(&"mine".to_owned()),
        "a window read after a delivered post whose request was dropped did not show it: \
         {answered:?}"
    );
    assert_eq!(
        h.chat.count("fetch_recent_dated"),
        2,
        "{:?}",
        h.chat.calls()
    );
}

#[tokio::test(start_paused = true)]
async fn a_timeline_answer_replayed_from_the_memo_does_not_put_back_an_older_copy() {
    // Review of #278: a newest timeline answer replayed from the few seconds' memory was
    // remembered again with its original start, over the newer copy a later window read had put in
    // read-ahead memory. Speech preparation and summaries then served the text from before an edit
    // nobody pushed.
    let h = harness();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let held_reply = || {
        let held = h
            .state
            .read_ahead
            .recall(&channel, None, &["11".to_owned()]);
        held["11"].content.clone()
    };
    let poll = timeline("view=main&reason=poll");
    assert_eq!(get(&h.app, &poll, READ_TOKEN).await.0, StatusCode::OK);
    // Edited upstream, and not pushed.
    h.upstream.messages.lock().unwrap()[1].content = "reply, edited".to_owned();
    let window = format!("/api/v1/channels/{WRITE_CHANNEL}/messages");
    let (status, body) = get(&h.app, &window, READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(contents(&body).contains(&"reply, edited".to_owned()));
    assert_eq!(held_reply(), "reply, edited", "fixture");
    // Inside the memo: answered from the kept read, which began before the edit.
    assert_eq!(get(&h.app, &poll, READ_TOKEN).await.0, StatusCode::OK);
    assert_eq!(
        h.chat.count("fetch_timeline"),
        1,
        "fixture: the poll was not answered from the memo"
    );
    assert_eq!(
        held_reply(),
        "reply, edited",
        "a memo replay put an older copy back over a newer one in read-ahead memory"
    );
}
