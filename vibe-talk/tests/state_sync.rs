//! `#2280 vibe-talk-state-sync-audit`, step S0: no page request awaits a provider read.
//!
//! The principle: the page talks only to this server, and this server answers page requests
//! from its own state; upstream reads run on the server's own schedule behind the answer.
//! Counting upstream calls cannot tell an awaited call from one begun behind an answer, so
//! each case here holds the next provider call at [`CountingChat`] and requires the page's
//! exact request, driven through the real router, to complete while that call is still held.
//! A route that awaits upstream cannot complete until the hold is released, at any host load;
//! the only clock below is `WAIT`, a hang backstop that turns that case into a failure
//! instead of a stuck suite. Nothing here asserts an answer was fast.
//!
//! Every route the router serves is classified below (`classification`): asserted here
//! (`Held`/`Zero`), local with no upstream await (`Local`), awaited today with its bound and
//! the audit step that removes it (`Awaited`/`NotChat`/`WriteAwaited`), or out of page scope
//! (`Excluded`). A new route with no entry fails, and an awaited entry with no bound or step
//! fails: fixing a route means moving its entry to an asserted class in the same change.
//! Conditions of an asserted route that still go provider-first (a view the cache holds
//! nothing of, above all) are listed in `CONDITIONS` the same way, and the cold-timeline test
//! proves the hold can tell them apart. Model and document/vendor upstreams do not pass
//! through `ChatClient`, so this file cannot hold them; they stay `NotChat`/`Awaited` until
//! their step adds a double at that seam. Writes are classified, not behaviourally asserted
//! here: S0 gates page reads, and each later step asserts the family it changes.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::cache::RefreshKey;
use vibe_talk::chat::{ChatClient, ChatError};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::live::Subscription;
use vibe_talk::model::{ChannelId, MessageId};
use vibe_talk::state::AppState;
use vibe_talk::testing::{
    self, CountingChat, ManualClock, TempDir, TimelineChat, READ_CHANNEL, WRITE_TOKEN,
};
use vibe_talk::threads::TimelineView;

/// 2026-08-18T12:00:00Z, as in `message_cache_serve`: fixture messages a minute apart.
const T0: i64 = 1_787_054_400_000;
const SECOND: i64 = 1_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
/// Hang backstop only (see module docs): the same twenty seconds `message_cache_serve`
/// allows its writer and shadow to settle. Never a latency assertion.
const WAIT: Duration = Duration::from_secs(20);

async fn call(state: &AppState, uri: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("the router answers");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn ok(state: &AppState, uri: &str, token: &str) -> Value {
    let (status, body) = call(state, uri, token).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

fn path(channel: &str, rest: &str) -> String {
    format!("/api/v1/channels/{channel}{rest}")
}

fn url_encode(raw: &str) -> String {
    raw.chars()
        .flat_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => vec![c],
            _ => format!("%{:02X}", u32::from(c)).chars().collect(),
        })
        .collect()
}

/// Wait for every shadow evaluation to be counted, and for the cache's writer.
async fn settled(state: &AppState) {
    assert!(
        state.message_cache.shadow_settled(WAIT).await,
        "every evaluation was counted"
    );
    if let Some(cache) = state.message_cache.cache() {
        assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    }
}

/// The texts a timeline or window answer carries, oldest first.
fn texts(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| {
            message["content"]
                .as_str()
                .expect("content is text")
                .to_owned()
        })
        .collect()
}

/// A server counting (and, on request, holding) every upstream call over a timeline fake,
/// with a message cache behind it: twelve messages a minute apart in the read channel.
struct Deployment {
    state: AppState,
    fake: Arc<FakeDiscord>,
    counting: Arc<CountingChat>,
    clock: Arc<ManualClock>,
    ids: Vec<MessageId>,
    _dir: TempDir,
}

async fn deployment(tag: &str) -> Deployment {
    let dir = TempDir::new(tag);
    let (mut state, fake, _voice) = testing::state_from_toml(&testing::config_toml());
    let read = ChannelId(READ_CHANNEL.to_owned());
    let ids = (0..12)
        .map(|n| fake.seed_at(&read, "Ada", &format!("message {n}"), T0 + n * MINUTE))
        .collect();
    let timeline: Arc<dyn ChatClient> = Arc::new(TimelineChat::new(fake.clone()));
    let counting = Arc::new(CountingChat::new(timeline));
    state.replace_chat(counting.clone());
    let clock = ManualClock::at(T0 + HOUR);
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    Deployment {
        state,
        fake,
        counting,
        clock,
        ids,
        _dir: dir,
    }
}

impl Deployment {
    /// Fill the read channel's `main` from a write-token timeline read of its newest page.
    async fn fill_timeline(&self) {
        ok(
            &self.state,
            &path(READ_CHANNEL, "/timeline?view=main&limit=50"),
            WRITE_TOKEN,
        )
        .await;
        settled(&self.state).await;
    }

    /// Fill the read channel's `main` from a write-token window read.
    async fn fill_window(&self) {
        ok(
            &self.state,
            &path(READ_CHANNEL, "/messages?limit=50"),
            WRITE_TOKEN,
        )
        .await;
        settled(&self.state).await;
    }

    /// Fill one thread from a write-token thread read; returns the provider page that
    /// filled it, so a test can tell a seeding miss from a cache miss.
    async fn fill_thread(&self, root: &MessageId) -> Value {
        let body = ok(
            &self.state,
            &path(
                READ_CHANNEL,
                &format!("/timeline?view=thread&thread_id={root}&limit=50"),
            ),
            WRITE_TOKEN,
        )
        .await;
        settled(&self.state).await;
        body
    }
}

/// Drive `uri` with the write token (the page's scope: serving requires it) while the next
/// upstream call of any method is held, and require the answer before that call is released.
///
/// Either no upstream call begins at all, or one begins behind the answer: both complete
/// here. An answer that awaits upstream completes in neither arm and fails on `WAIT`.
async fn answers_while_held(d: &Deployment, uri: &str) -> (StatusCode, Value) {
    let before = d.counting.calls().len();
    let mut held = d.counting.hold_next_any();
    let state = d.state.clone();
    let asked = uri.to_owned();
    let mut task = tokio::spawn(async move { call(&state, &asked, WRITE_TOKEN).await });
    let answered = tokio::select! {
        done = &mut task => done.expect("the request ran"),
        entered = &mut held.entered => {
            entered.expect("the held call's entered side is open");
            tokio::time::timeout(WAIT, &mut task)
                .await
                .unwrap_or_else(|_| panic!("{uri} answered only with upstream: still waiting while the provider call is held; calls begun: {:?}", calls_since(d, before)))
                .expect("the request ran")
        }
    };
    let _ = held.release.send(());
    settled(&d.state).await;
    d.counting.clear_holds();
    answered
}

/// The cache-first stand-in's words (`ops::CACHE_FIRST`): an answer from this server's
/// copy at once, as against a slow stand-in's "did not answer in time", which means the
/// request first waited out its provider deadline. Held cases require the former (or a
/// proven answer with no `refresh_failed` at all): completing while held after a deadline
/// wait is still a wait, and no clock is needed to tell them apart.
fn assert_answered_at_once(body: &Value) {
    match body.get("refresh_failed") {
        None => {}
        Some(why) => assert_eq!(
            why.as_str(),
            Some("Answered from this server's copy while the chat service is read."),
            "answered only after a provider deadline, not from state at once: {body}"
        ),
    }
}

/// As [`answers_while_held`], requiring success.
async fn ok_while_held(d: &Deployment, uri: &str) -> Value {
    let (status, body) = answers_while_held(d, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

/// Upstream calls begun since `before` (a `calls().len()` taken earlier), by method.
fn calls_since(d: &Deployment, before: usize) -> Vec<String> {
    d.counting.calls()[before..]
        .iter()
        .map(|call| call.method.to_owned())
        .collect()
}

#[tokio::test]
async fn timeline_newest_thread_and_delta_answer_while_the_provider_is_held() {
    let d = deployment("state-sync-timeline").await;
    // One thread on the fifth message, so its view can be primed like `main`.
    let root = d.ids[5].clone();
    d.fake.seed_reply(
        &ChannelId(READ_CHANNEL.to_owned()),
        "Ada",
        "thread reply",
        &root,
    );
    // A provider newest read primes the cache and hands back the forward cursor a poll
    // continues from (as `message_cache_serve`'s `forward_cursor`); serving is still off.
    let first = ok(
        &d.state,
        &path(READ_CHANNEL, "/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    settled(&d.state).await;
    let after = first["next_after"]
        .as_str()
        .expect("a forward cursor")
        .to_owned();
    let filled = d.fill_thread(&root).await;
    assert!(
        texts(&filled).iter().any(|text| text == "thread reply"),
        "the fill read itself carries the reply: {filled}"
    );
    // Too old for the proven-serve rule, as in the cache-first cases there: what answers
    // now is the cache-first stand-in, from this server's copy at once.
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);

    let body = ok_while_held(
        &d,
        &path(READ_CHANNEL, "/timeline?view=main&limit=50&reason=enter"),
    )
    .await;
    assert_answered_at_once(&body);
    assert_eq!(texts(&body).len(), 12, "{body}");

    // Catch-up from the provider page's cursor, as a poll continues: also at once. (A
    // forward read from a stand-in page's own cursor has no landed read to take and still
    // waits out its deadline: CONDITIONS, S1. Observed while writing this file; named
    // there until S1 asserts it at once.)
    let delta = ok_while_held(
        &d,
        &path(
            READ_CHANNEL,
            &format!(
                "/timeline?view=main&limit=50&after={}&reason=visible",
                url_encode(&after)
            ),
        ),
    )
    .await;
    assert_answered_at_once(&delta);
    assert_eq!(texts(&delta).len(), 12, "{delta}");

    // The page's own thread-pane read carries its reason (`side`); untagged, this same
    // request is not a page read the cache answers first and waits instead.
    let thread = ok_while_held(
        &d,
        &path(
            READ_CHANNEL,
            &format!("/timeline?view=thread&thread_id={root}&limit=50&reason=side"),
        ),
    )
    .await;
    assert_answered_at_once(&thread);
    // The held thread copy here is the root observation: this fake's reply carries no
    // thread field for the cache to file it under the thread by (the fill assert above
    // proves the provider page itself carries it; thread-shaped fills are covered by the
    // Slack deployments in `message_cache_serve`). What S0 pins is the at-once answer.
    let seen = texts(&thread);
    assert!(
        seen.iter().any(|text| text == "message 5"),
        "the held root answers the thread view: {thread}"
    );
}

#[tokio::test]
async fn a_step_back_from_a_served_page_answers_while_the_provider_is_held() {
    // As `message_cache_serve`'s cursor walk, through the counting hold: a proven page
    // first (hours later, serving on), then its own cursor back into held history.
    let d = deployment("state-sync-older").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    d.clock.advance(SECOND);
    let body = ok_while_held(&d, &path(READ_CHANNEL, "/timeline?view=main&limit=5")).await;
    let cursor = body["next_before"]
        .as_str()
        .expect("a backward cursor")
        .to_owned();
    assert!(
        cursor.starts_with(vibe_talk::timeline_served::PREFIX),
        "the server's own cursor, not a provider's: {body}"
    );
    let older = ok_while_held(
        &d,
        &path(
            READ_CHANNEL,
            &format!("/timeline?view=main&limit=5&before={}", url_encode(&cursor)),
        ),
    )
    .await;
    assert_answered_at_once(&older);
    assert_eq!(
        texts(&older),
        [
            "message 2",
            "message 3",
            "message 4",
            "message 5",
            "message 6"
        ],
        "{older}"
    );
}

#[tokio::test]
async fn window_answers_while_the_provider_is_held() {
    // The fresh-window priming of `message_cache_serve`'s window case: fill, five
    // seconds on (the head is still fresh), serving on.
    let d = deployment("state-sync-window").await;
    d.fill_window().await;
    d.clock.advance(5 * SECOND);
    d.state.message_cache.set_serve(true);

    let window = ok_while_held(&d, &path(READ_CHANNEL, "/messages?limit=5")).await;
    assert_answered_at_once(&window);
    assert_eq!(
        texts(&window),
        [
            "message 7",
            "message 8",
            "message 9",
            "message 10",
            "message 11"
        ],
        "{window}"
    );
}

#[tokio::test]
async fn search_never_goes_upstream() {
    let d = deployment("state-sync-search").await;
    d.fill_timeline().await;
    d.state.message_cache.set_serve(true);

    // Search is stronger than held: no upstream call at all, behind included.
    for uri in [
        path(READ_CHANNEL, "/search?q=message"),
        "/api/v1/search?q=message".to_owned(),
    ] {
        let before = d.counting.calls().len();
        let hits = ok_while_held(&d, &uri).await;
        assert!(hits["hits"].is_array(), "{uri}: {hits}");
        assert_eq!(
            hits["upstream"].as_str(),
            Some("none"),
            "{uri}: search says where it looked: {hits}"
        );
        assert_eq!(
            calls_since(&d, before),
            Vec::<String>::new(),
            "{uri} went upstream"
        );
    }
}

/// Concrete `Zero` cases: (route template, request URI). Each must complete (a missing
/// object may 404: that refusal is this server's own) with no upstream call begun, behind
/// answers included. `every_router_route_is_classified` keeps this list and
/// `classification` in step, so a case cannot silently stop covering its route.
fn zero_cases() -> Vec<(&'static str, String)> {
    let ch = READ_CHANNEL;
    vec![
        ("/api/v1/documents", "/api/v1/documents?limit=5".into()),
        (
            "/api/v1/documents/search",
            "/api/v1/documents/search?q=body".into(),
        ),
        (
            "/api/v1/documents/{key}",
            format!(
                "/api/v1/documents/{}",
                url_encode("github:octo/widgets@main:docs/report.md")
            ),
        ),
        ("/api/v1/channels", "/api/v1/channels".into()),
        ("/api/v1/agent-tools", "/api/v1/agent-tools".into()),
        ("/api/v1/client-config", "/api/v1/client-config".into()),
        ("/api/v1/voice-agent", "/api/v1/voice-agent".into()),
        (
            "/api/v1/voice-recap/{id}",
            "/api/v1/voice-recap/missing".into(),
        ),
        ("/api/v1/transcripts", "/api/v1/transcripts?limit=5".into()),
        (
            "/api/v1/channels/{channel_id}/catchup/preview",
            path(ch, "/catchup/preview?view=flat"),
        ),
        (
            "/api/v1/channels/{channel_id}/catchup/record",
            path(ch, "/catchup/record"),
        ),
        ("/api/v1/glossary/terms", "/api/v1/glossary/terms".into()),
        ("/api/v1/channels/{channel_id}/drafts", path(ch, "/drafts")),
        (
            "/api/v1/post-proposals",
            "/api/v1/post-proposals?wait=0&sent_after=0".into(),
        ),
        ("/api/v1/conversations", "/api/v1/conversations".into()),
        (
            "/api/v1/conversations/{conversation_id}",
            "/api/v1/conversations/conv_01?records=1".into(),
        ),
        (
            "/api/v1/conversations/{conversation_id}/replay",
            "/api/v1/conversations/conv_01/replay".into(),
        ),
        (
            "/api/v1/transcript",
            "/api/v1/transcript?limit=5&records=1".into(),
        ),
        ("/api/v1/inbox", "/api/v1/inbox".into()),
        (
            "/api/v1/channels/{channel_id}/upstream-clear",
            path(ch, "/upstream-clear"),
        ),
        (
            "/api/v1/channels/{channel_id}/history",
            path(ch, "/history"),
        ),
        ("/api/v1/voice-settings", "/api/v1/voice-settings".into()),
        ("/api/v1/client-settings", "/api/v1/client-settings".into()),
        ("/api/v1/voice-prompt", "/api/v1/voice-prompt".into()),
        (
            "/api/v1/voice-post-prefix",
            "/api/v1/voice-post-prefix".into(),
        ),
        ("/api/v1/channels/{channel_id}/pins", path(ch, "/pins")),
        (
            "/api/v1/channels/{channel_id}/thread-pins",
            path(ch, "/thread-pins"),
        ),
        ("/api/v1/labels", "/api/v1/labels".into()),
        (
            "/api/v1/channels/{channel_id}/bookmark",
            path(ch, "/bookmark"),
        ),
    ]
}

#[tokio::test]
async fn store_only_reads_begin_no_upstream_call() {
    let d = deployment("state-sync-zero").await;
    d.fill_timeline().await;
    d.state.message_cache.set_serve(true);
    for (template, uri) in zero_cases() {
        let before = d.counting.calls().len();
        let (status, body) = tokio::time::timeout(WAIT, call(&d.state, &uri, WRITE_TOKEN))
            .await
            .unwrap_or_else(|_| panic!("{template} ({uri}) did not answer"));
        settled(&d.state).await;
        assert!(
            status.as_u16() < 500,
            "{template} ({uri}) answered {status}: {body}"
        );
        assert_eq!(
            calls_since(&d, before),
            Vec::<String>::new(),
            "{template} ({uri}) went upstream"
        );
    }
}

#[tokio::test]
async fn a_cold_timeline_waits_for_the_provider_the_allowlisted_exception() {
    // The discriminator's other side, and `CONDITIONS`' cold-view entry made executable:
    // with nothing held there is nothing to answer with, so the read must still be waiting
    // while its provider call is held, then complete from upstream once released. When the
    // cold step (S1) answers from state instead, this test is rewritten with it.
    let d = deployment("state-sync-cold").await;
    d.state.message_cache.set_serve(true);
    let mut held = d.counting.hold_next_any();
    let state = d.state.clone();
    let mut task = tokio::spawn(async move {
        call(
            &state,
            &path(READ_CHANNEL, "/timeline?view=main&limit=50&reason=enter"),
            WRITE_TOKEN,
        )
        .await
    });
    tokio::time::timeout(WAIT, &mut held.entered)
        .await
        .expect("the cold read reached the provider")
        .expect("the held call's entered side is open");
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(
        !task.is_finished(),
        "nothing to answer with: the cold read waits for the provider"
    );
    held.release.send(()).expect("the read is still out");
    let (status, body) = tokio::time::timeout(WAIT, &mut task)
        .await
        .expect("the cold read answered after release")
        .expect("the request ran");
    assert_eq!(status, StatusCode::OK, "cold timeline: {body}");
    assert_eq!(texts(&body).len(), 12, "{body}");
    assert!(body.get("refresh_failed").is_none(), "{body}");
    settled(&d.state).await;
    d.counting.clear_holds();
}

/// Wait until no upstream read of one view is running, by claiming its refresh slot as
/// `message_cache_serve`'s `refreshed` does: the condition is the claim coming free,
/// polled, with `WAIT` only as a hang backstop.
async fn claim_free(state: &AppState, channel: &str, view: TimelineView, limit: u16) {
    let key = || RefreshKey::timeline(ChannelId(channel.to_owned()), view, None, limit);
    tokio::time::timeout(WAIT, async {
        while state.message_cache.claim_refresh(key()).is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the read behind the answer finished");
    settled(state).await;
}

/// Require `id` to arrive on `subscription` by live event: the retry's publication,
/// with no page read made to collect it.
async fn live_arrival(subscription: &mut Subscription, id: &MessageId) {
    tokio::time::timeout(WAIT, async {
        loop {
            let live = subscription.receiver.recv().await.expect("a live arrival");
            if live.message.id == *id {
                break;
            }
        }
    })
    .await
    .expect("arrived by live event without a page read");
}

#[tokio::test]
async fn a_failed_refresh_behind_a_stand_in_retries_on_the_schedule_not_the_next_read() {
    // `#2280` S2: an ordinary failure (not a budget refusal) of the read behind a
    // cache-first answer used to stand until the next page read. The schedule, aged
    // here instead of slept through, is what retries it.
    let d = deployment("state-sync-behind-retry").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    let read = ChannelId(READ_CHANNEL.to_owned());
    let late = d
        .fake
        .seed_at(&read, "Ada", "arrived late", T0 + 30 * MINUTE);
    let mut subscription = d.state.live.subscribe(&read, None);
    while subscription.receiver.try_recv().is_ok() {}

    d.counting.fail_next(
        "fetch_timeline",
        None,
        ChatError::Transport("down for one read".to_owned()),
    );
    let body = ok(
        &d.state,
        &path(READ_CHANNEL, "/timeline?view=main&limit=50&reason=enter"),
        WRITE_TOKEN,
    )
    .await;
    assert_answered_at_once(&body);
    assert!(
        !texts(&body).iter().any(|text| text == "arrived late"),
        "the stand-in is the copy as it stood: {body}"
    );

    // No page read from here: the failed behind read retries when due and publishes.
    claim_free(&d.state, READ_CHANNEL, TimelineView::Main, 50).await;
    d.state.refresh_schedule.age(Duration::from_secs(300));
    live_arrival(&mut subscription, &late).await;
}

/// A timeline provider whose next Flat page, once armed, leaves one thread's replies
/// out and records the thread as owed — Slack `flat()` under its budget, at the
/// `ChatClient` seam this file may stub. Later Flat pages are whole.
struct OwedOnce {
    inner: Arc<TimelineChat>,
    owed: std::sync::Mutex<Option<MessageId>>,
}

impl OwedOnce {
    fn arm(&self, root: MessageId) {
        *self
            .owed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(root);
    }
}

#[async_trait::async_trait]
impl ChatClient for OwedOnce {
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn supports_threading(&self) -> bool {
        self.inner.supports_threading()
    }

    async fn identity(&self) -> Result<vibe_talk::chat::ChatIdentity, ChatError> {
        self.inner.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<vibe_talk::model::Message>, ChatError> {
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &vibe_talk::threads::TimelineRequest,
    ) -> Result<vibe_talk::threads::TimelinePage, ChatError> {
        let mut page = self.inner.fetch_timeline(channel, request).await?;
        if request.view == TimelineView::Flat {
            let armed = self
                .owed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(root) = armed {
                page.messages
                    .retain(|message| message.reply_to.as_ref() != Some(&root));
                page.owed_threads.push(root);
            }
        }
        Ok(page)
    }

    async fn fetch_thread_message(
        &self,
        channel: &ChannelId,
        thread_id: &str,
        message_id: &MessageId,
    ) -> Result<vibe_talk::model::Message, ChatError> {
        self.inner
            .fetch_thread_message(channel, thread_id, message_id)
            .await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<vibe_talk::model::Message, ChatError> {
        self.inner.post_message(channel, content, reply_to).await
    }
}

/// As [`deployment`], counting over an [`OwedOnce`] timeline provider.
async fn owed_deployment(tag: &str) -> (Deployment, Arc<OwedOnce>) {
    let dir = TempDir::new(tag);
    let (mut state, fake, _voice) = testing::state_from_toml(&testing::config_toml());
    let read = ChannelId(READ_CHANNEL.to_owned());
    let ids = (0..12)
        .map(|n| fake.seed_at(&read, "Ada", &format!("message {n}"), T0 + n * MINUTE))
        .collect();
    let owed = Arc::new(OwedOnce {
        inner: Arc::new(TimelineChat::new(fake.clone())),
        owed: std::sync::Mutex::new(None),
    });
    let provider: Arc<dyn ChatClient> = owed.clone();
    let counting = Arc::new(CountingChat::new(provider));
    state.replace_chat(counting.clone());
    let clock = ManualClock::at(T0 + HOUR);
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    (
        Deployment {
            state,
            fake,
            counting,
            clock,
            ids,
            _dir: dir,
        },
        owed,
    )
}

#[tokio::test]
async fn replies_an_all_read_left_out_as_owed_arrive_without_another_page_read() {
    // `#2278`, through ops: the behind All read records its dropped thread as owed;
    // the scheduled re-read — not the next poll — is what brings its replies in.
    let (d, owed) = owed_deployment("state-sync-owed").await;
    let read = ChannelId(READ_CHANNEL.to_owned());
    let root = d.ids[5].clone();
    // Prime All from the provider (serving still off), root alone in its thread.
    ok(
        &d.state,
        &path(READ_CHANNEL, "/timeline?view=flat&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    settled(&d.state).await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);

    let late = d.fake.seed_reply(&read, "Ada", "owed reply", &root);
    let mut subscription = d.state.live.subscribe(&read, None);
    while subscription.receiver.try_recv().is_ok() {}
    owed.arm(root.clone());
    let body = ok(
        &d.state,
        &path(READ_CHANNEL, "/timeline?view=flat&limit=50&reason=poll"),
        WRITE_TOKEN,
    )
    .await;
    assert_answered_at_once(&body);
    assert!(
        !texts(&body).iter().any(|text| text == "owed reply"),
        "the stand-in predates the reply: {body}"
    );

    // No page read from here: the owed re-read, aged due, publishes the reply live.
    claim_free(&d.state, READ_CHANNEL, TimelineView::Flat, 50).await;
    d.state.refresh_schedule.age(Duration::from_secs(300));
    live_arrival(&mut subscription, &late).await;
}

/// How one route stands under the principle today. `bound` is the worst wait the audit
/// evidenced in code (never a latency claim about a run); `step` is the audit step that
/// removes the wait, or `JUSTIFY` where it may stay with that bound kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// Asserted above: answers while the provider is held (`answers_while_held`).
    Held,
    /// Asserted above: begins no upstream call at all (`zero_cases`).
    Zero,
    /// No upstream await: this server's store, settings, telemetry, or an enqueue.
    /// Classification-frozen in S0; the step that changes a family asserts it.
    Local,
    /// Awaits a chat-provider or model call before answering, on some condition.
    Awaited {
        /// Worst evidenced wait, in words.
        bound: &'static str,
        /// Audit step (`S1`..`S6`) or `JUSTIFY`.
        step: &'static str,
    },
    /// Awaits an upstream that is not `ChatClient` (documents, voice vendor): this file
    /// cannot hold it, so it is named until its step adds a double at that seam.
    NotChat {
        /// Worst evidenced wait, in words.
        bound: &'static str,
        /// Audit step (`S1`..`S6`) or `JUSTIFY`.
        step: &'static str,
    },
    /// An upstream write (or a join on one) awaited before answering.
    WriteAwaited {
        /// Worst evidenced wait, in words.
        bound: &'static str,
        /// Audit step (`S1`..`S6`) or `JUSTIFY`.
        step: &'static str,
    },
    /// Not a page data read: no state-sync verdict is owed here.
    Excluded {
        /// Why it is out of page scope.
        reason: &'static str,
    },
}

#[derive(Debug)]
struct Entry {
    method: &'static str,
    path: &'static str,
    class: Class,
}

const fn e(method: &'static str, path: &'static str, class: Class) -> Entry {
    Entry {
        method,
        path,
        class,
    }
}

/// Every route the router serves, classified. Order follows `scope_first`'s inventory;
/// `every_router_route_is_classified` checks it against the router source itself, so the
/// order is for readers, not for the test.
fn classification() -> Vec<Entry> {
    vec![
        e(
            "GET",
            "/healthz",
            Class::Excluded {
                reason: "liveness, non-page",
            },
        ),
        e(
            "GET",
            "/readyz",
            Class::Excluded {
                reason: "readiness, non-page",
            },
        ),
        e(
            "GET",
            "/",
            Class::Excluded {
                reason: "page shell static",
            },
        ),
        e(
            "GET",
            "/voice",
            Class::Excluded {
                reason: "page shell redirect",
            },
        ),
        e(
            "GET",
            "/voice/",
            Class::Excluded {
                reason: "page shell redirect",
            },
        ),
        e(
            "GET",
            "/voice.js",
            Class::Excluded {
                reason: "page shell static",
            },
        ),
        e(
            "GET",
            "/contract.js",
            Class::Excluded {
                reason: "page shell static",
            },
        ),
        e(
            "GET",
            "/voice.css",
            Class::Excluded {
                reason: "page shell static",
            },
        ),
        e(
            "GET",
            "/style.css",
            Class::Excluded {
                reason: "page shell static",
            },
        ),
        e(
            "GET",
            "/manifest.webmanifest",
            Class::Excluded {
                reason: "page shell static",
            },
        ),
        e(
            "GET",
            "/icons/{name}",
            Class::Excluded {
                reason: "page shell static",
            },
        ),
        e(
            "GET",
            "/assets/{name}",
            Class::Excluded {
                reason: "page shell static",
            },
        ),
        e("GET", "/api/v1/documents", Class::Zero),
        e(
            "POST",
            "/api/v1/documents/fetch",
            Class::NotChat {
                bound: "document upstream 45s",
                step: "S5",
            },
        ),
        e(
            "POST",
            "/api/v1/documents/fetch-recent",
            Class::NotChat {
                bound: "serial to 20 x 45s",
                step: "S5",
            },
        ),
        e("GET", "/api/v1/documents/search", Class::Zero),
        e("DELETE", "/api/v1/documents/cache", Class::Local),
        e("GET", "/api/v1/documents/{key}", Class::Zero),
        e(
            "GET",
            "/api/v1/documents/{key}/freshness",
            Class::NotChat {
                bound: "document upstream 45s",
                step: "S5",
            },
        ),
        e(
            "POST",
            "/api/v1/documents/{key}/speech/prepare",
            Class::Local,
        ),
        e("GET", "/api/v1/channels", Class::Zero),
        e(
            "GET",
            "/api/v1/activity",
            Class::Awaited {
                bound: "heads 1s + watch 2.5s, cold 8s",
                step: "S3",
            },
        ),
        e("GET", "/api/v1/agent-tools", Class::Zero),
        e("GET", "/api/v1/client-config", Class::Zero),
        e(
            "POST",
            "/api/v1/live/events",
            Class::Excluded {
                reason: "adapter ingest credential",
            },
        ),
        e(
            "POST",
            "/api/v1/live/hints",
            Class::Excluded {
                reason: "adapter ingest credential",
            },
        ),
        e(
            "POST",
            "/api/v1/live/tail",
            Class::Excluded {
                reason: "adapter ingest credential",
            },
        ),
        e(
            "GET",
            "/api/v1/live/tail",
            Class::Excluded {
                reason: "adapter ingest credential",
            },
        ),
        e(
            "GET",
            "/api/v1/diagnostics",
            Class::Awaited {
                bound: "admin deadline budget; numeric bound to record",
                step: "JUSTIFY",
            },
        ),
        e(
            "GET",
            "/api/v1/signed-url",
            Class::NotChat {
                bound: "vendor mint, no bound in handler",
                step: "S6",
            },
        ),
        e(
            "GET",
            "/api/v1/read-aloud/voices",
            Class::NotChat {
                bound: "vendor list, no bound in handler",
                step: "S6",
            },
        ),
        e("POST", "/api/v1/pair", Class::Local),
        e("POST", "/api/v1/pair/redeem", Class::Local),
        e(
            "GET",
            "/api/v1/voice-session",
            Class::NotChat {
                bound: "live vendor mint",
                step: "JUSTIFY",
            },
        ),
        e(
            "GET",
            "/api/v1/voice-socket",
            Class::Excluded {
                reason: "websocket relay; live vendor dial 10s",
            },
        ),
        e("GET", "/api/v1/voice-agent", Class::Zero),
        e(
            "POST",
            "/api/v1/voice-recap",
            Class::Awaited {
                bound: "own-session poll about 5s, no chat read",
                step: "S6",
            },
        ),
        e("DELETE", "/api/v1/voice-recap", Class::Local),
        e("GET", "/api/v1/voice-recap/{id}", Class::Zero),
        e("DELETE", "/api/v1/voice-recap/{id}", Class::Local),
        e("POST", "/api/v1/voice-timing", Class::Local),
        e("POST", "/api/v1/voice-health", Class::Local),
        e("POST", "/api/v1/voice-end", Class::Local),
        e("POST", "/api/v1/scroll-trace", Class::Local),
        e("POST", "/api/v1/freeze-report", Class::Local),
        e("POST", "/api/v1/voice-heard", Class::Local),
        e("GET", "/api/v1/channels/{channel_id}/messages", Class::Held),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/messages/{message_id}",
            Class::Awaited {
                bound: "provider lookup, unbounded in ops",
                step: "S3",
            },
        ),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/messages/{message_id}/summary",
            Class::Awaited {
                bound: "provider window + model on miss",
                step: "S4",
            },
        ),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/page",
            Class::Awaited {
                bound: "span reads, unbounded in ops",
                step: "S3",
            },
        ),
        e("GET", "/api/v1/channels/{channel_id}/timeline", Class::Held),
        e("GET", "/api/v1/channels/{channel_id}/search", Class::Held),
        e("GET", "/api/v1/search", Class::Held),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/count",
            Class::Awaited {
                bound: "span reads, unbounded in ops",
                step: "S3",
            },
        ),
        e("GET", "/api/v1/transcripts", Class::Zero),
        e(
            "GET",
            "/api/v1/transcripts/{channel_id}/{thread_id}",
            Class::Awaited {
                bound: "via timeline provider arms",
                step: "S3",
            },
        ),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/digest",
            Class::Awaited {
                bound: "window provider read",
                step: "S3",
            },
        ),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/recap",
            Class::Awaited {
                bound: "walk to 50 pages, each awaited",
                step: "S3",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/catchup",
            Class::Awaited {
                bound: "model awaited, 60s floor per call",
                step: "S4",
            },
        ),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/catchup/preview",
            Class::Zero,
        ),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/catchup/record",
            Class::Zero,
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/catchup/messages",
            Class::Awaited {
                bound: "model follow-up awaited",
                step: "S4",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/catchup/update",
            Class::Awaited {
                bound: "model update awaited",
                step: "S4",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/catchup/new-summary",
            Class::Awaited {
                bound: "model summary awaited",
                step: "S4",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/glossary/lookup",
            Class::Awaited {
                bound: "cache retrieval + model on miss",
                step: "S4",
            },
        ),
        e("GET", "/api/v1/glossary/terms", Class::Zero),
        e("GET", "/api/v1/channels/{channel_id}/drafts", Class::Zero),
        e("PUT", "/api/v1/channels/{channel_id}/drafts", Class::Local),
        e(
            "DELETE",
            "/api/v1/channels/{channel_id}/drafts",
            Class::Local,
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/resolve",
            Class::Awaited {
                bound: "recent provider read",
                step: "S3",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/reply",
            Class::WriteAwaited {
                bound: "sync post + verify by default; accepted arm enqueues",
                step: "S6",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/reply/outcome",
            Class::WriteAwaited {
                bound: "bounded watch 25s",
                step: "JUSTIFY",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/ask",
            Class::Awaited {
                bound: "agent stub awaited",
                step: "S4",
            },
        ),
        e("GET", "/api/v1/post-proposals", Class::Zero),
        e(
            "POST",
            "/api/v1/post-proposals",
            Class::WriteAwaited {
                bound: "may post when approval allows direct send",
                step: "JUSTIFY",
            },
        ),
        e(
            "POST",
            "/api/v1/post-proposals/commit",
            Class::WriteAwaited {
                bound: "owner-confirmed post",
                step: "JUSTIFY",
            },
        ),
        e("POST", "/api/v1/post-proposals/cancel", Class::Local),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/stream",
            Class::Excluded {
                reason: "long-lived stream; sync subscribe",
            },
        ),
        e(
            "GET",
            "/api/v1/live/stream",
            Class::Excluded {
                reason: "long-lived stream; sync subscribe",
            },
        ),
        e("GET", "/api/v1/conversations", Class::Zero),
        e("DELETE", "/api/v1/conversations", Class::Local),
        e(
            "GET",
            "/api/v1/conversations/{conversation_id}",
            Class::Zero,
        ),
        e(
            "DELETE",
            "/api/v1/conversations/{conversation_id}",
            Class::Local,
        ),
        e(
            "POST",
            "/api/v1/conversations/{conversation_id}/turns",
            Class::Local,
        ),
        e(
            "GET",
            "/api/v1/conversations/{conversation_id}/replay",
            Class::Zero,
        ),
        e("GET", "/api/v1/transcript", Class::Zero),
        e("DELETE", "/api/v1/storage", Class::Local),
        e("PUT", "/api/v1/message-cache", Class::Local),
        e("DELETE", "/api/v1/message-cache", Class::Local),
        e("GET", "/api/v1/inbox", Class::Zero),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/messages/{message_id}/speak",
            Class::Awaited {
                bound: "cold provider read unbounded, then vendor",
                step: "S6",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/speech/prepare",
            Class::Awaited {
                bound: "1.5s + 1.5s; window lock across fetch",
                step: "S6",
            },
        ),
        e(
            "GET",
            "/api/v1/speech/{ticket}",
            Class::NotChat {
                bound: "vendor stream; join bounded 25s",
                step: "S6",
            },
        ),
        e("POST", "/api/v1/speech/{ticket}/timing", Class::Local),
        e("POST", "/api/v1/speech/{ticket}/failure", Class::Local),
        e(
            "GET",
            "/api/v1/speech/{ticket}/failure",
            Class::Excluded {
                reason: "ticket-gated local failure read",
            },
        ),
        e(
            "POST",
            "/api/v1/channels",
            Class::Awaited {
                bound: "registration lock across register + probe",
                step: "S6",
            },
        ),
        e(
            "GET",
            "/api/v1/channel-directory",
            Class::Awaited {
                bound: "cold discovery; repeats kept 120s",
                step: "S5",
            },
        ),
        e(
            "DELETE",
            "/api/v1/channels/{channel_id}",
            Class::Awaited {
                bound: "registration lock queue",
                step: "S6",
            },
        ),
        e(
            "DELETE",
            "/api/v1/channels/{channel_id}/hidden",
            Class::Awaited {
                bound: "registration lock queue",
                step: "S6",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/dismiss",
            Class::Awaited {
                bound: "through arm awaits window; plain dismiss local",
                step: "S3",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/restore",
            Class::Local,
        ),
        e("POST", "/api/v1/own-read/apply", Class::Local),
        e("POST", "/api/v1/channels/{channel_id}/read", Class::Local),
        e("DELETE", "/api/v1/channels/{channel_id}/read", Class::Local),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/upstream-read",
            Class::WriteAwaited {
                bound: "cold scope read + mark; floor lock",
                step: "S5",
            },
        ),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/upstream-clear",
            Class::Zero,
        ),
        e(
            "PUT",
            "/api/v1/channels/{channel_id}/upstream-clear",
            Class::Local,
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/upstream-clear/now",
            Class::Awaited {
                bound: "provider read + push wait 45s",
                step: "S5",
            },
        ),
        e("GET", "/api/v1/channels/{channel_id}/history", Class::Zero),
        e("PUT", "/api/v1/channels/{channel_id}/history", Class::Local),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/history/prioritize",
            Class::Local,
        ),
        e(
            "DELETE",
            "/api/v1/channels/{channel_id}/history/prioritize",
            Class::Local,
        ),
        e("PUT", "/api/v1/channels/{channel_id}/alias", Class::Local),
        e(
            "DELETE",
            "/api/v1/channels/{channel_id}/alias",
            Class::Local,
        ),
        e("PUT", "/api/v1/noise-rules", Class::Local),
        e("GET", "/api/v1/voice-settings", Class::Zero),
        e("PUT", "/api/v1/voice-settings", Class::Local),
        e("GET", "/api/v1/client-settings", Class::Zero),
        e("PUT", "/api/v1/client-settings", Class::Local),
        e("GET", "/api/v1/voice-prompt", Class::Zero),
        e("PUT", "/api/v1/voice-prompt", Class::Local),
        e("GET", "/api/v1/voice-post-prefix", Class::Zero),
        e("PUT", "/api/v1/voice-post-prefix", Class::Local),
        e("DELETE", "/api/v1/voice-post-prefix", Class::Local),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/not-noise",
            Class::Local,
        ),
        e("GET", "/api/v1/channels/{channel_id}/pins", Class::Zero),
        e(
            "PUT",
            "/api/v1/channels/{channel_id}/pins/{message_id}",
            Class::Local,
        ),
        e(
            "DELETE",
            "/api/v1/channels/{channel_id}/pins/{message_id}",
            Class::Local,
        ),
        e(
            "GET",
            "/api/v1/channels/{channel_id}/thread-pins",
            Class::Zero,
        ),
        e(
            "PUT",
            "/api/v1/channels/{channel_id}/thread-pins/{thread_id}",
            Class::Local,
        ),
        e(
            "DELETE",
            "/api/v1/channels/{channel_id}/thread-pins/{thread_id}",
            Class::Local,
        ),
        e("GET", "/api/v1/labels", Class::Zero),
        e("POST", "/api/v1/label-groups", Class::Local),
        e("PUT", "/api/v1/label-groups/{id}", Class::Local),
        e("DELETE", "/api/v1/label-groups/{id}", Class::Local),
        e("GET", "/api/v1/channels/{channel_id}/bookmark", Class::Zero),
        e(
            "PUT",
            "/api/v1/channels/{channel_id}/bookmark",
            Class::Local,
        ),
        e(
            "DELETE",
            "/api/v1/channels/{channel_id}/bookmark",
            Class::Local,
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/fake-seed",
            Class::Excluded {
                reason: "test instances only",
            },
        ),
        e(
            "POST",
            "/api/v1/channels/{channel_id}/thread-hues",
            Class::Local,
        ),
    ]
}

/// Conditions of an asserted route that still go provider-first, each with its bound and
/// the step that removes it. The asserted cases above prime the route out of these
/// conditions; naming them here is what keeps that priming honest.
const CONDITIONS: &[(&str, &str, &str)] = &[
    (
        "cold view: cache holds nothing of the view",
        "stand-in wait (enter 1.5s, else 10s default) then unbounded in ops",
        "S1",
    ),
    (
        "timeline with serving off or a read-scope caller",
        "provider-first, no stand-in timer",
        "S1",
    ),
    (
        "place and reply-place timeline reads",
        "10s default (0 under recent limit pressure), then unbounded if nothing held",
        "S1",
    ),
    (
        "threads list without a way back in cache",
        "provider-first, as cold",
        "S1",
    ),
    (
        "forward (after) read whose cursor no landed read matches",
        "stand-in wait 10s default, then held rows without delta proof",
        "S1",
    ),
    (
        "older step, reason older, first step of a walk",
        "joins its spawned walk for 1.5s, then answers pending",
        "S1",
    ),
    (
        "recent window refresh failure",
        "next catch-up re-reads; rate-limit backoff 30s",
        "S3",
    ),
    (
        "catch-up preview in thread scope (step fill)",
        "one provider timeline page per poll",
        "S4",
    ),
    (
        "side and counts timeline reads, cold",
        "fails fast at the 10s default instead of awaiting further",
        "JUSTIFY",
    ),
    (
        "refused-class provider errors (access, cursor, refusal)",
        "error returned even when rows are held",
        "JUSTIFY",
    ),
    (
        "post-proposals with wait above zero",
        "bounded watch to 25s (asserted case uses wait=0)",
        "JUSTIFY",
    ),
];

/// Route templates the held cases above exercise. Kept in step with `Class::Held` by
/// `every_router_route_is_classified`.
const HELD_TEMPLATES: &[(&str, &str)] = &[
    ("GET", "/api/v1/channels/{channel_id}/timeline"),
    ("GET", "/api/v1/channels/{channel_id}/messages"),
    ("GET", "/api/v1/channels/{channel_id}/search"),
    ("GET", "/api/v1/search"),
];

/// Every `(METHOD, path)` the router source registers with a literal path. The same
/// deliberately small reading as `scope_first::routes_in_router_source`, failing closed on
/// a registration shape it cannot read: state-sync completeness must not depend on a
/// second hand-kept list staying in step by goodwill.
fn routes_in_router_source() -> BTreeSet<(String, String)> {
    let whole = include_str!("../src/http/mod.rs");
    let (outer, source) = whole
        .split_once("\nfn routes(")
        .expect("src/http/mod.rs has a `fn routes`");
    assert!(
        outer.matches(".fallback_service(").count() == 1
            && outer.contains(".fallback_service(routes(state.clone()))")
            && !outer.contains(".route("),
        "outside `fn routes`, src/http/mod.rs may only wrap `routes` in the outer router"
    );
    for unread in [
        ".route_service(",
        ".nest(",
        ".nest_service(",
        ".merge(",
        ".fallback_service(",
    ] {
        assert!(
            !source.contains(unread) && (unread == ".fallback_service(" || !outer.contains(unread)),
            "src/http/mod.rs registers routes with `{unread}`, which this inventory cannot read"
        );
    }
    let mut found = BTreeSet::new();
    let mut constants = 0;
    let mut rest = source;
    while let Some(at) = rest.find(".route(") {
        rest = &rest[at + ".route(".len()..];
        let mut depth = 1;
        let end = rest
            .char_indices()
            .find_map(|(index, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(index);
                        }
                    }
                    _ => {}
                }
                None
            })
            .expect("every .route( call closes");
        let arguments = &rest[..end];
        let Some(literal) = arguments.trim_start().strip_prefix('"') else {
            assert!(
                arguments
                    .trim_start()
                    .starts_with("crate::mcp::transport::MCP_PATH"),
                "a route whose path is neither a literal nor MCP_PATH: {arguments}"
            );
            constants += 1;
            continue;
        };
        let path = &literal[..literal.find('"').expect("a closed literal")];
        for method in [
            "get", "post", "put", "delete", "patch", "head", "options", "trace", "connect", "any",
            "on",
        ] {
            let needle = format!("{method}(");
            let mut from = 0;
            while let Some(offset) = arguments[from..].find(&needle) {
                let index = from + offset;
                let before = arguments[..index].chars().next_back();
                if !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                    found.insert((method.to_ascii_uppercase(), path.to_owned()));
                }
                from = index + needle.len();
            }
        }
    }
    assert_eq!(
        constants, 1,
        "exactly one route, /mcp, is registered by constant (excluded: model transport)"
    );
    found
}

fn step_is_named(step: &str) -> bool {
    matches!(step, "S1" | "S2" | "S3" | "S4" | "S5" | "S6" | "JUSTIFY")
}

#[test]
fn every_router_route_is_classified() {
    let listed: BTreeSet<(String, String)> = classification()
        .iter()
        .map(|entry| (entry.method.to_owned(), entry.path.to_owned()))
        .collect();
    assert_eq!(
        classification().len(),
        listed.len(),
        "a route is classified twice"
    );
    let served = routes_in_router_source();
    let unlisted: Vec<_> = served.difference(&listed).collect();
    let stale: Vec<_> = listed.difference(&served).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "routes served but not classified: {unlisted:?}; classified but not served: {stale:?}"
    );

    // Asserted classes and the cases that assert them, in step both ways.
    let held: BTreeSet<(String, String)> = classification()
        .iter()
        .filter(|entry| entry.class == Class::Held)
        .map(|entry| (entry.method.to_owned(), entry.path.to_owned()))
        .collect();
    let held_cases: BTreeSet<(String, String)> = HELD_TEMPLATES
        .iter()
        .map(|(method, path)| ((*method).to_owned(), (*path).to_owned()))
        .collect();
    assert_eq!(held, held_cases, "Held routes and held cases differ");
    let zero: BTreeSet<String> = classification()
        .iter()
        .filter(|entry| entry.class == Class::Zero)
        .map(|entry| {
            assert_eq!(entry.method, "GET", "Zero is a read class: {entry:?}");
            entry.path.to_owned()
        })
        .collect();
    let zero_covered: BTreeSet<String> = zero_cases()
        .iter()
        .map(|(template, _)| (*template).to_owned())
        .collect();
    assert_eq!(zero, zero_covered, "Zero routes and zero cases differ");

    // No unnamed debt: every awaited entry carries its bound and its step, every
    // exclusion its reason.
    for entry in classification() {
        let whereabouts = format!("{} {}", entry.method, entry.path);
        match entry.class {
            Class::Awaited { bound, step }
            | Class::NotChat { bound, step }
            | Class::WriteAwaited { bound, step } => {
                assert!(!bound.is_empty(), "{whereabouts}: awaited with no bound");
                assert!(
                    step_is_named(step),
                    "{whereabouts}: step {step:?} is not S1..S6 or JUSTIFY"
                );
            }
            Class::Excluded { reason } => {
                assert!(!reason.is_empty(), "{whereabouts}: excluded with no reason");
            }
            Class::Held | Class::Zero | Class::Local => {}
        }
    }
}

#[test]
fn every_condition_has_a_bound_and_a_step() {
    let mut names = BTreeSet::new();
    for (name, bound, step) in CONDITIONS {
        assert!(names.insert(*name), "condition listed twice: {name}");
        assert!(!bound.is_empty(), "{name}: no bound");
        assert!(
            step_is_named(step),
            "{name}: step {step:?} is not S1..S6 or JUSTIFY"
        );
    }
    assert!(
        CONDITIONS
            .iter()
            .any(|(name, _, _)| name.starts_with("cold view")),
        "the cold-view condition the cold test executes is listed"
    );
    // S2 removed the behind-failure and flat-owed conditions by fixing them; the
    // retry cases above are their proof now, not entries here.
}
