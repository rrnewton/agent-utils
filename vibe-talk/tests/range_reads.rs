//! remote-ops-audit W10 `#287 bulk-reads`: time-range reads of a channel.
//!
//! The first test pins what `/page` and `/count` answer, and every upstream read they make, on a
//! provider that overrides nothing but the three methods every provider must write. Its fixture
//! was recorded by the code as it stood BEFORE `ChatClient::fetch_range` existed, so it is the
//! proof that a provider without a range read of its own still gets those answers byte for byte,
//! at the same cost, once the routes read through the trait's default. Rewrite it only for a
//! change that is MEANT to alter what such a provider is asked or answers:
//! `VIBE_TALK_UPDATE_FIXTURES=1 cargo test --test range_reads`.

mod slack_fake;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, system, thread, ts, Fake, FakeState};
use tower::ServiceExt as _;
use vibe_talk::chat::{
    ChatClient, ChatError, ChatIdentity, RangeOrder, RangeRead, ReadCaps, ReadScope, Support,
    TimeRange,
};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, ChannelInfo, Message, MessageId};
use vibe_talk::providers::{ChatRouter, ProviderEntry};
use vibe_talk::testing::{READ_CHANNEL, READ_TOKEN};

/// 2026-08-17T20:53:20Z, the first message of the fixture.
const BASE_MS: i64 = 1_787_000_000_000;
/// One message a minute.
const MINUTE_MS: i64 = 60_000;

/// A provider that writes only what the trait requires, and logs every page it is asked for.
///
/// Everything else runs the trait's default body on THIS client, which is the case the fixture
/// pins: a provider with no override of its own.
struct Plain {
    inner: Arc<FakeDiscord>,
    log: Mutex<Vec<String>>,
}

impl Plain {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().expect("log"))
    }
}

#[async_trait]
impl ChatClient for Plain {
    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.inner.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.log.lock().expect("log").push(format!(
            "fetch_page limit={limit} before={} after={}",
            before.map_or("-", MessageId::as_str),
            after.map_or("-", MessageId::as_str),
        ));
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.inner.post_message(channel, content, reply_to).await
    }
}

fn iso(ms: i64) -> String {
    vibe_talk::clock::iso_from_ms(ms).replace(':', "%3A")
}

/// The channel the fixture reads: four hours at one message a minute, every seventh a placeholder
/// the default noise rules call read, and messages on and either side of the edges the cases use.
fn seed(discord: &FakeDiscord) {
    let channel = ChannelId(READ_CHANNEL.to_owned());
    for i in 0..240_i64 {
        let author = ["ada", "grace", "linus"][usize::try_from(i % 3).expect("small")];
        let content = if i % 7 == 3 {
            "Working…".to_owned()
        } else {
            format!("m{i}")
        };
        discord.seed_at(&channel, author, &content, BASE_MS + i * MINUTE_MS);
    }
    let since = since_ms();
    let until = until_ms();
    discord.seed_at(&channel, "ada", "just before since", since - 1);
    discord.seed_at(&channel, "grace", "just before until", until - 1);
    discord.seed_at(&channel, "linus", "exactly at until", until);
}

/// Exactly on the 31st message.
fn since_ms() -> i64 {
    BASE_MS + 30 * MINUTE_MS
}

/// Half way between two messages.
fn until_ms() -> i64 {
    BASE_MS + 100 * MINUTE_MS + 30_000
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", format!("Bearer {READ_TOKEN}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router responds");
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

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/range_reads_default.json")
}

/// Every case: what was asked, what came back, and every upstream page it took.
async fn record() -> Value {
    let (mut state, discord) = vibe_talk::testing::state();
    seed(&discord);
    let plain = Arc::new(Plain {
        inner: Arc::clone(&discord),
        log: Mutex::new(Vec::new()),
    });
    let namespace = state.config.providers[0].namespace();
    state.replace_providers(Arc::new(ChatRouter::single(
        Arc::clone(&plain) as Arc<dyn ChatClient>,
        &namespace,
        0,
    )));
    let router = vibe_talk::http::router(state);
    let base = format!("/api/v1/channels/{READ_CHANNEL}");
    let (since, until) = (since_ms(), until_ms());
    let newest = BASE_MS + 239 * MINUTE_MS;
    let m50 = MessageId::at_time_ms(BASE_MS + 50 * MINUTE_MS);

    let mut queries = vec![
        format!("page?since={}&limit=3", iso(since)),
        format!("page?since={}&until={}", iso(since), iso(since + 90_000)),
        format!("page?since={}", iso(newest + 1)),
        format!(
            "page?since={}&until={}&limit=50",
            iso(BASE_MS - MINUTE_MS),
            iso(BASE_MS + 5 * MINUTE_MS)
        ),
        format!("page?since={}&until={}", iso(until - 1), iso(until + 1)),
        "page?limit=4".to_owned(),
        format!("page?before={}&limit=4", m50.as_str()),
        format!("count?since={}", iso(since)),
        format!("count?since={}&cap=50", iso(since)),
        "count?cap=120".to_owned(),
        "count".to_owned(),
        format!("count?since={}", iso(newest + 1)),
        format!("count?since={}", iso(BASE_MS - MINUTE_MS)),
    ];
    // A whole range walk, each step from the `next_since` the one before handed back.
    let mut walk = Some(format!(
        "page?since={}&until={}&limit=5",
        iso(since),
        iso(until)
    ));
    let mut cases = Vec::new();
    let mut steps = 0;
    while let Some(query) = walk.take() {
        let (case, body) = run(&router, &base, &query, &plain, &discord).await;
        cases.push(case);
        steps += 1;
        walk = body["next_since"]
            .as_str()
            .filter(|_| steps < 20)
            .map(|next| {
                format!(
                    "page?since={}&until={}&limit=5",
                    next.replace(':', "%3A"),
                    iso(until)
                )
            });
    }
    assert!(steps > 3, "the walk took several steps: {steps}");
    for query in queries.drain(..) {
        cases.push(run(&router, &base, &query, &plain, &discord).await.0);
    }
    Value::Array(cases)
}

async fn run(
    router: &axum::Router,
    base: &str,
    query: &str,
    plain: &Plain,
    discord: &FakeDiscord,
) -> (Value, Value) {
    let sent_before = discord.fetch_count();
    let (status, body) = get(router, &format!("{base}/{query}")).await;
    let pages = plain.take();
    let sent = discord.fetch_count() - sent_before;
    let case = json!({
        "query": query,
        "status": status.as_u16(),
        "upstream_requests": sent,
        "pages": pages,
        "body": body,
    });
    (case, body)
}

#[tokio::test]
async fn a_provider_without_a_range_read_gets_todays_page_and_count_answers_at_todays_cost() {
    let recorded = record().await;
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&recorded).expect("serializes")
    );
    let path = fixture_path();
    if std::env::var_os("VIBE_TALK_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
        std::fs::write(&path, &text).expect("write the fixture");
        return;
    }
    let held = std::fs::read_to_string(&path).expect("the recorded fixture");
    let held: Value = serde_json::from_str(&held).expect("the fixture is JSON");
    let (held, recorded) = (
        held.as_array().expect("cases"),
        recorded.as_array().expect("cases"),
    );
    assert_eq!(held.len(), recorded.len(), "the same cases");
    for (was, now) in held.iter().zip(recorded) {
        assert_eq!(
            was, now,
            "{} no longer answers or reads upstream as it did before range reads existed",
            was["query"]
        );
    }
    // The fixture is not vacuous: reads happened, and both kinds of answer are in it.
    let reads: u64 = held
        .iter()
        .map(|case| case["upstream_requests"].as_u64().expect("count"))
        .sum();
    assert!(reads > held.len() as u64, "{reads} reads");
    assert!(held.iter().all(|case| case["status"] == 200));
    assert!(held
        .iter()
        .any(|case| case["body"]["at_least"] == true && case["body"]["cap"] == 50));
    assert!(held
        .iter()
        .any(|case| case["body"]["has_more"] == true && case["body"]["next_since"].is_string()));
}

/// One range read a provider was asked for: channel, order, limit and cursor.
type Asked = (ChannelId, RangeOrder, u16, Option<String>);

/// A provider that reads by time itself, and records each range read it is asked for.
struct Native {
    asked: Mutex<Vec<Asked>>,
}

#[async_trait]
impl ChatClient for Native {
    fn provider_name(&self) -> &str {
        "Native"
    }

    fn read_caps(&self, _channel: &ChannelId) -> ReadCaps {
        ReadCaps {
            range: Support::Native,
            page_rows: 20,
        }
    }

    async fn fetch_range(
        &self,
        channel: &ChannelId,
        _scope: &ReadScope,
        _range: TimeRange,
        order: RangeOrder,
        limit: u16,
        cursor: Option<&str>,
    ) -> Result<RangeRead, ChatError> {
        self.asked.lock().expect("asked").push((
            channel.clone(),
            order,
            limit,
            cursor.map(str::to_owned),
        ));
        Ok(RangeRead {
            exhausted: true,
            ..RangeRead::default()
        })
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        Ok(ChatIdentity {
            id: "1".to_owned(),
            username: "native".to_owned(),
        })
    }

    async fn fetch_page(
        &self,
        _: &ChannelId,
        _: u16,
        _: Option<&MessageId>,
        _: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        Err(ChatError::Refused(
            "a range read never pages here".to_owned(),
        ))
    }

    async fn post_message(
        &self,
        _: &ChannelId,
        _: &str,
        _: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused("read only".to_owned()))
    }
}

fn routed(id: &ChannelId, provider: &str) -> ChannelInfo {
    ChannelInfo {
        id: id.clone(),
        label: provider.to_owned(),
        writable: false,
        alias: None,
        added: false,
        provider: Some(provider.to_owned()),
    }
}

/// remote-ops-audit W10: the router forwards a range read, and the question whether one is cheap,
/// to the channel's own provider, and each is timed under that provider.
#[tokio::test]
async fn the_router_asks_each_channels_own_provider_whether_and_how_it_reads_by_time() {
    let native = Arc::new(Native {
        asked: Mutex::new(Vec::new()),
    });
    let plain = Arc::new(FakeDiscord::new());
    let (on_native, on_plain) = (ChannelId("10".to_owned()), ChannelId("20".to_owned()));
    plain.register_channel(&on_plain);
    plain.seed_at(&on_plain, "ada", "inside", BASE_MS + 1_000);
    plain.seed_at(&on_plain, "ada", "past the end", BASE_MS + 9_000);
    let router = ChatRouter::new(
        vec![
            ProviderEntry {
                key: "native".to_owned(),
                namespace: "native".to_owned(),
                client: Arc::clone(&native) as Arc<dyn ChatClient>,
                live_poll_seconds: 0,
            },
            ProviderEntry {
                key: "plain".to_owned(),
                namespace: "plain".to_owned(),
                client: Arc::clone(&plain) as Arc<dyn ChatClient>,
                live_poll_seconds: 0,
            },
        ],
        Some("plain"),
    );
    router.replace_routes(&[routed(&on_native, "native"), routed(&on_plain, "plain")]);
    let nowhere = ChannelId("30".to_owned());

    assert_eq!(router.read_caps(&on_native).range, Support::Native);
    assert_eq!(router.read_caps(&on_native).page_rows, 20);
    assert_eq!(router.read_caps(&on_plain), ReadCaps::default());
    assert_eq!(
        router.read_caps(&nowhere),
        ReadCaps::default(),
        "an unrouted channel is not claimed to be cheap"
    );

    let range = TimeRange {
        since_ms: BASE_MS,
        until_ms: Some(BASE_MS + 5_000),
    };
    let read = router
        .fetch_range(
            &on_native,
            &ReadScope::Main,
            range,
            RangeOrder::NewestFirst,
            40,
            Some("c"),
        )
        .await
        .expect("native read");
    assert!(read.exhausted);
    assert_eq!(
        native.asked.lock().expect("asked").clone(),
        [(
            on_native.clone(),
            RangeOrder::NewestFirst,
            40,
            Some("c".to_owned())
        )]
    );
    assert_eq!(plain.fetch_count(), 0);

    let read = router
        .fetch_range(
            &on_plain,
            &ReadScope::Main,
            range,
            RangeOrder::OldestFirst,
            40,
            None,
        )
        .await
        .expect("plain read");
    assert_eq!(read.messages.len(), 1, "trimmed to the window: {read:?}");
    assert_eq!(read.messages[0].content, "inside");
    assert_eq!(plain.fetch_count(), 1);
    assert_eq!(native.asked.lock().expect("asked").len(), 1);

    let error = router
        .fetch_range(
            &nowhere,
            &ReadScope::Main,
            range,
            RangeOrder::OldestFirst,
            40,
            None,
        )
        .await
        .expect_err("unrouted");
    assert!(matches!(error.cause(), ChatError::Refused(_)), "{error}");

    let report = router.profile().report();
    for provider in &report.providers {
        let timed = provider
            .operations
            .iter()
            .filter(|row| row.name == "fetch_range")
            .map(|row| row.count)
            .sum::<u64>();
        assert_eq!(timed, 1, "{} timed its range read", provider.provider);
    }
}

const SLACK_CH: &str = "C0000MAIN1";

fn slack_channel() -> ChannelId {
    ChannelId(SLACK_CH.to_owned())
}

/// Milliseconds of a fixture `ts` that falls on a whole millisecond.
fn slack_ms(seconds: i64) -> i64 {
    (slack_fake::BASE_SECS + seconds) * 1000
}

/// A conversation of `older` messages one second apart, then `inside` more from second 10 000:
/// the window a test reads starts on the first of those.
fn slack_history(older: i64, inside: i64, newer: i64) -> FakeState {
    let mut state = FakeState::default();
    let mut list = Vec::new();
    for i in 0..older {
        list.push(profiled(&ts(i, 0), "UADA00001", "Ada", &format!("old {i}")));
    }
    for i in 0..inside {
        list.push(profiled(
            &ts(10_000 + i, 0),
            "UADA00001",
            "Ada",
            &format!("in {i}"),
        ));
    }
    for i in 0..newer {
        list.push(profiled(
            &ts(20_000 + i, 0),
            "UADA00001",
            "Ada",
            &format!("new {i}"),
        ));
    }
    state.history.insert(SLACK_CH.to_owned(), list);
    state
}

fn contents(read: &RangeRead) -> Vec<String> {
    read.messages.iter().map(|m| m.content.clone()).collect()
}

/// remote-ops-audit W10 proof. Newest first, a window with 150 messages in it and 400 before it
/// costs one `conversations.history` call per 20 rows it holds, every one pinned to the window,
/// and is exhausted only because Slack said so.
#[tokio::test]
async fn slack_reads_a_window_newest_first_in_one_call_per_page_it_holds() {
    let fake = Fake::start(slack_history(400, 150, 0), 20).await;
    let client = fake.client();
    assert_eq!(
        client.read_caps(&slack_channel()),
        ReadCaps {
            range: Support::Native,
            page_rows: 20,
        }
    );
    let read = client
        .fetch_range(
            &slack_channel(),
            &ReadScope::Main,
            TimeRange {
                since_ms: slack_ms(10_000),
                until_ms: None,
            },
            RangeOrder::NewestFirst,
            500,
            None,
        )
        .await
        .expect("read");
    assert_eq!(
        contents(&read),
        (0..150).map(|i| format!("in {i}")).collect::<Vec<_>>(),
        "every message of the window, oldest first, and none before it"
    );
    assert!(read.exhausted, "Slack answered has_more: false");
    assert_eq!(read.next, None);
    let calls = fake.requests("conversations.history");
    assert_eq!(calls.len(), 8, "150 rows at 20 a page: {calls:?}");
    let latest = calls[0].get("latest").cloned().expect("latest is pinned");
    for call in &calls {
        assert_eq!(
            call.get("oldest").map(String::as_str),
            Some(ts(10_000, 0).as_str())
        );
        assert_eq!(
            call.get("latest"),
            Some(&latest),
            "one window for every page"
        );
        assert_eq!(call.get("inclusive").map(String::as_str), Some("true"));
    }
}

/// A step stops at what it was asked for and hands back Slack's cursor; the next step reads the
/// same pinned window from there, and the walk is the window, once.
#[tokio::test]
async fn slack_continues_a_window_from_its_own_cursor_without_reading_anything_twice() {
    let fake = Fake::start(slack_history(10, 150, 0), 20).await;
    let client = fake.client();
    let range = TimeRange {
        since_ms: slack_ms(10_000),
        until_ms: None,
    };
    let mut cursor: Option<String> = None;
    let mut walked: Vec<String> = Vec::new();
    let mut steps = 0;
    loop {
        let read = client
            .fetch_range(
                &slack_channel(),
                &ReadScope::Main,
                range,
                RangeOrder::NewestFirst,
                50,
                cursor.as_deref(),
            )
            .await
            .expect("read");
        steps += 1;
        assert!(read.messages.len() <= 50);
        let mut step = contents(&read);
        step.extend(walked);
        walked = step;
        if read.exhausted {
            assert_eq!(read.next, None);
            break;
        }
        cursor = Some(read.next.expect("a step short of the end continues"));
        assert!(steps < 10, "the walk ends");
    }
    assert_eq!(steps, 3);
    assert_eq!(
        walked,
        (0..150).map(|i| format!("in {i}")).collect::<Vec<_>>()
    );
    // Each step of 50 is pages of 20, 20 and the 10 still wanted: never a row asked for and
    // dropped, which Slack's cursor would then step over.
    let calls = fake.requests("conversations.history");
    let asked: Vec<&str> = calls
        .iter()
        .map(|call| call.get("limit").map_or("", String::as_str))
        .collect();
    assert_eq!(
        asked,
        ["50", "30", "10", "50", "30", "10", "50", "30", "10"]
    );
    assert!(calls
        .iter()
        .all(|call| call.get("latest") == calls[0].get("latest")));

    // A cursor is for the read that issued it.
    let error = client
        .fetch_range(
            &slack_channel(),
            &ReadScope::Main,
            TimeRange {
                since_ms: slack_ms(9_000),
                until_ms: None,
            },
            RangeOrder::NewestFirst,
            50,
            cursor.as_deref(),
        )
        .await
        .expect_err("another window");
    assert!(matches!(error.cause(), ChatError::Refused(_)), "{error}");
}

/// remote-ops-audit W10: a step with nothing real in it is not the end of the window. Slack's
/// `has_more` is the only proof, however short the step.
#[tokio::test]
async fn slack_never_takes_a_short_step_for_the_end_of_the_window() {
    let mut state = FakeState::default();
    let mut list: Vec<Value> = (0..5)
        .map(|i| profiled(&ts(i, 0), "UADA00001", "Ada", &format!("real {i}")))
        .collect();
    list.extend((0..60).map(|i| system(&ts(100 + i, 0), "channel_join")));
    state.history.insert(SLACK_CH.to_owned(), list);
    let fake = Fake::start(state, 20).await;
    let client = fake.client();
    let range = TimeRange {
        since_ms: slack_ms(0),
        until_ms: None,
    };
    let read = client
        .fetch_range(
            &slack_channel(),
            &ReadScope::Main,
            range,
            RangeOrder::NewestFirst,
            5,
            None,
        )
        .await
        .expect("read");
    assert!(read.messages.is_empty(), "{:?}", contents(&read));
    assert!(!read.exhausted, "nothing real in a step proves nothing");
    assert_eq!(
        fake.calls("conversations.history"),
        2,
        "one page and one more"
    );
    let mut cursor = read.next;
    let mut found = Vec::new();
    for _ in 0..20 {
        let Some(at) = cursor.take() else { break };
        let read = client
            .fetch_range(
                &slack_channel(),
                &ReadScope::Main,
                range,
                RangeOrder::NewestFirst,
                5,
                Some(&at),
            )
            .await
            .expect("read");
        found.splice(0..0, contents(&read));
        if read.exhausted {
            break;
        }
        cursor = read.next;
    }
    assert_eq!(
        found,
        (0..5).map(|i| format!("real {i}")).collect::<Vec<_>>()
    );
}

/// remote-ops-audit W10: oldest first, a window with an end is read up to that end and no
/// further, so a span costs what it holds, not what came after it.
#[tokio::test]
async fn slack_reads_a_span_oldest_first_without_reading_past_its_end() {
    let fake = Fake::start(slack_history(10, 30, 400), 20).await;
    let client = fake.client();
    let range = TimeRange {
        since_ms: slack_ms(10_000),
        until_ms: Some(slack_ms(20_000)),
    };
    let read = client
        .fetch_range(
            &slack_channel(),
            &ReadScope::Main,
            range,
            RangeOrder::OldestFirst,
            11,
            None,
        )
        .await
        .expect("read");
    assert_eq!(
        contents(&read),
        (0..11).map(|i| format!("in {i}")).collect::<Vec<_>>()
    );
    assert!(!read.exhausted, "19 more are in the window");
    let calls = fake.requests("conversations.history");
    assert_eq!(
        calls.len(),
        2,
        "30 rows at 20 a page, none of the 400 after: {calls:?}"
    );
    for call in &calls {
        assert_eq!(
            call.get("latest").map(String::as_str),
            Some(ts(20_000, 0).as_str()),
            "bounded by the window's end"
        );
    }
    let mut walked = contents(&read);
    let mut cursor = read.next;
    while let Some(at) = cursor.take() {
        let read = client
            .fetch_range(
                &slack_channel(),
                &ReadScope::Main,
                range,
                RangeOrder::OldestFirst,
                11,
                Some(&at),
            )
            .await
            .expect("read");
        walked.extend(contents(&read));
        if read.exhausted {
            break;
        }
        cursor = read.next;
    }
    assert_eq!(
        walked,
        (0..30).map(|i| format!("in {i}")).collect::<Vec<_>>()
    );
    assert!(fake
        .requests("conversations.history")
        .iter()
        .all(|call| call.get("latest").map(String::as_str) == Some(ts(20_000, 0).as_str())));
}

/// A thread is read by time through `conversations.replies`, in its own order, from its own
/// cursor; a whole history with replies is refused rather than guessed.
#[tokio::test]
async fn slack_reads_a_threads_window_through_its_replies() {
    let mut state = FakeState::default();
    let mut root = profiled(&ts(0, 0), "UADA00001", "Ada", "root");
    let replies: Vec<(String, String)> = (0..50)
        .map(|i| (ts(100 + i, 0), format!("reply {i}")))
        .collect();
    thread(&mut state, SLACK_CH, &mut root, &replies);
    state.history.insert(SLACK_CH.to_owned(), vec![root]);
    let fake = Fake::start(state, 20).await;
    let client = fake.client();
    let range = TimeRange {
        since_ms: slack_ms(110),
        until_ms: Some(slack_ms(140)),
    };
    let scope = ReadScope::Thread(ts(0, 0));
    let read = client
        .fetch_range(
            &slack_channel(),
            &scope,
            range,
            RangeOrder::OldestFirst,
            100,
            None,
        )
        .await
        .expect("read");
    assert_eq!(
        contents(&read),
        (10..40).map(|i| format!("reply {i}")).collect::<Vec<_>>()
    );
    assert!(read.exhausted);
    let calls = fake.requests("conversations.replies");
    assert_eq!(calls.len(), 2, "30 replies at 20 a page: {calls:?}");
    for call in &calls {
        assert_eq!(call.get("ts").map(String::as_str), Some(ts(0, 0).as_str()));
        assert_eq!(
            call.get("oldest").map(String::as_str),
            Some(ts(110, 0).as_str())
        );
        assert!(call.contains_key("latest"));
    }
    assert_eq!(fake.calls("conversations.history"), 0);

    let error = client
        .fetch_range(
            &slack_channel(),
            &ReadScope::Flat,
            range,
            RangeOrder::OldestFirst,
            100,
            None,
        )
        .await
        .expect_err("refused");
    assert!(matches!(error.cause(), ChatError::Refused(_)), "{error}");
}
