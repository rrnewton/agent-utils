//! remote-ops-audit W11 `#287 bulk-reads`: looking messages up by id.
//!
//! The first test pins what every lookup by id answers, and every upstream call it makes, on a
//! provider that overrides nothing but the methods every provider must write and a thread lookup
//! of one message. Its fixture was recorded by the code as it stood BEFORE
//! `ChatClient::fetch_messages` existed, so it is the proof that a provider without a lookup of
//! several ids of its own still gets those answers byte for byte, at the same cost, once the
//! callers read through the trait. Rewrite it only for a change that is MEANT to alter what such a
//! provider is asked or answers:
//! `VIBE_TALK_UPDATE_FIXTURES=1 cargo test --test message_lookups`.

mod slack_fake;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse as _;
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, system, ts, FakeState};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity, Lookup, ReadCaps, Support};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::providers::ChatRouter;
use vibe_talk::read_reason::ReadReason;
use vibe_talk::state::AppState;
use vibe_talk::testing::{CountingChat, READ_CHANNEL, READ_TOKEN, WRITE_TOKEN};
use vibe_talk::threads::MessageThread;

/// 2026-08-17T20:53:20Z, the first message of the fixture.
const BASE_MS: i64 = 1_787_000_000_000;
/// One message a minute.
const MINUTE_MS: i64 = 60_000;
/// The thread every thread lookup here is in.
const THREAD: &str = "opaque/thread-T";

/// A message of [`THREAD`], `n` minutes after the first.
fn thread_message(id: &str, n: i64, content: &str) -> Message {
    let at = BASE_MS + n * MINUTE_MS;
    Message {
        id: MessageId(id.to_owned()),
        channel_id: ChannelId(READ_CHANNEL.to_owned()),
        author: "grace".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: vibe_talk::clock::iso_from_ms(at),
        spoken_time: String::new(),
        reply_to: None,
        content: content.to_owned(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        thread: Some(MessageThread {
            id: THREAD.to_owned(),
            root_message_id: Some(MessageId("t0".to_owned())),
            is_root: id == "t0",
            reply_count: None,
            reply_count_exact: false,
        }),
        reactions: None,
        heading: None,
    }
}

/// The thread's messages: a root, a long reply in three parts, and short replies.
fn thread() -> Vec<Message> {
    let long = |tag: &str| format!("{tag} ").repeat(120);
    let mut messages = vec![thread_message("t0", 0, "the root of the thread")];
    messages.push(thread_message("t1", 1, &long("first part")));
    messages.push(thread_message("t2", 2, &long("second part")));
    messages.push(thread_message("t3", 3, "and the end of it"));
    for n in 4..12 {
        messages.push(thread_message(&format!("t{n}"), n, &format!("reply {n}")));
    }
    messages
}

/// A provider that writes only what the trait requires, a thread lookup of one message and an
/// upstream read mark, and logs every call that may reach upstream.
///
/// Everything else runs the trait's default body on THIS client, which is the case the fixture
/// pins: a provider with no lookup of several ids of its own.
struct Plain {
    inner: Arc<FakeDiscord>,
    threads: HashMap<String, Vec<Message>>,
    log: Mutex<Vec<String>>,
}

impl Plain {
    fn new(inner: Arc<FakeDiscord>) -> Self {
        Self {
            inner,
            threads: HashMap::from([(THREAD.to_owned(), thread())]),
            log: Mutex::new(Vec::new()),
        }
    }

    fn note(&self, line: String) {
        self.log.lock().expect("log").push(line);
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().expect("log"))
    }
}

#[async_trait]
impl ChatClient for Plain {
    fn supports_threading(&self) -> bool {
        true
    }

    fn supports_upstream_read_mark(&self) -> bool {
        true
    }

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
        self.note(format!(
            "fetch_page limit={limit} before={} after={}",
            before.map_or("-", MessageId::as_str),
            after.map_or("-", MessageId::as_str),
        ));
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn fetch_thread_message(
        &self,
        _channel: &ChannelId,
        thread_id: &str,
        message_id: &MessageId,
    ) -> Result<Message, ChatError> {
        self.note(format!(
            "fetch_thread_message thread={thread_id} id={}",
            message_id.as_str()
        ));
        self.threads
            .get(thread_id)
            .and_then(|messages| messages.iter().find(|m| m.id == *message_id))
            .cloned()
            .ok_or_else(|| ChatError::Status {
                status: 404,
                body: "no such message in that thread".to_owned(),
            })
    }

    async fn mark_read_upstream(
        &self,
        _channel: &ChannelId,
        through: &MessageId,
    ) -> Result<(), ChatError> {
        self.note(format!("mark_read_upstream through={}", through.as_str()));
        Ok(())
    }

    async fn post_message(
        &self,
        _channel: &ChannelId,
        _content: &str,
        _reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused("this test posts nothing".to_owned()))
    }
}

/// The channel's own messages: ten, a minute apart.
fn seed(discord: &FakeDiscord) -> Vec<MessageId> {
    let channel = ChannelId(READ_CHANNEL.to_owned());
    (0..10_i64)
        .map(|i| {
            discord.seed_at(
                &channel,
                ["ada", "linus"][usize::try_from(i % 2).expect("small")],
                &format!("main message {i}"),
                BASE_MS + 30 * MINUTE_MS + i * MINUTE_MS,
            )
        })
        .collect()
}

fn summaries_toml() -> String {
    format!(
        "{}\n[summaries]\nthreshold_chars = 400\ntarget_chars = 160\ncontext_messages = 3\n",
        vibe_talk::testing::config_toml()
    )
}

/// A server reading through `client`, routed as a single provider.
fn server(client: Arc<dyn ChatClient>) -> (AppState, axum::Router) {
    let (mut state, _discord, _store, _summarizer) =
        vibe_talk::testing::state_with_summarizer(&summaries_toml());
    let namespace = state.config.providers[0].namespace();
    state.replace_providers(Arc::new(ChatRouter::single(client, &namespace, 0)));
    let router = vibe_talk::http::router(state.clone());
    (state, router)
}

async fn call(
    router: &axum::Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("router");
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

/// The parts of an answer that differ from run to run, replaced by a fixed word.
fn steady(mut body: Value) -> Value {
    if let Some(prepared) = body.get_mut("prepared").and_then(Value::as_array_mut) {
        for entry in prepared {
            entry["url"] = json!("<ticket>");
        }
    }
    if body.get("generated_in_ms").is_some() {
        body["generated_in_ms"] = json!("<ms>");
    }
    body
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/message_lookups_default.json")
}

/// One request a case makes: who asks, how, where, and with what.
struct Ask {
    name: &'static str,
    token: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every caller that looks messages up by id, with the ids it is given found and not.
fn asks(main: &[MessageId]) -> Vec<Ask> {
    let base = format!("/api/v1/channels/{READ_CHANNEL}");
    let thread = THREAD.replace('/', "%2F");
    let ask = |name, token, method, uri: String, body| Ask {
        name,
        token,
        method,
        uri,
        body,
    };
    let rpc = |message_id: &str| {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "read_message",
                "arguments": { "channel_id": READ_CHANNEL, "message_id": message_id },
            },
        })
    };
    let eight: Vec<String> = (4..12).map(|n| format!("t{n}")).collect();
    vec![
        ask(
            "message by id, in the window",
            READ_TOKEN,
            "GET",
            format!("{base}/messages/{}", main[7].as_str()),
            None,
        ),
        ask(
            "message by id, not in the window",
            READ_TOKEN,
            "GET",
            format!("{base}/messages/404040404040"),
            None,
        ),
        ask(
            "message by id, in a thread",
            READ_TOKEN,
            "GET",
            format!("{base}/messages/t5?thread_id={thread}"),
            None,
        ),
        ask(
            "message by id, not in the thread",
            READ_TOKEN,
            "GET",
            format!("{base}/messages/t99?thread_id={thread}"),
            None,
        ),
        ask(
            "speech preparation of eight thread messages",
            READ_TOKEN,
            "POST",
            format!("{base}/speech/prepare"),
            Some(json!({ "ids": eight, "thread_id": THREAD })),
        ),
        ask(
            "speech preparation with one id not in the thread",
            READ_TOKEN,
            "POST",
            format!("{base}/speech/prepare"),
            Some(json!({ "ids": ["t4", "t99", "t5"], "thread_id": THREAD })),
        ),
        ask(
            "a thread summary of a row in three parts",
            WRITE_TOKEN,
            "GET",
            format!("{base}/messages/t1/summary?thread_id={thread}&with=t2,t3"),
            None,
        ),
        ask(
            "a thread summary of a part not in the thread",
            WRITE_TOKEN,
            "GET",
            format!("{base}/messages/t1/summary?thread_id={thread}&with=t2,t99"),
            None,
        ),
        ask(
            "an upstream read mark in a thread",
            WRITE_TOKEN,
            "POST",
            format!("{base}/upstream-read"),
            Some(json!({ "message_id": "t6", "thread_id": THREAD })),
        ),
        ask(
            "an upstream read mark through a message not in the thread",
            WRITE_TOKEN,
            "POST",
            format!("{base}/upstream-read"),
            Some(json!({ "message_id": "t99", "thread_id": THREAD })),
        ),
        ask(
            "read_message, in the window",
            READ_TOKEN,
            "POST",
            "/mcp".to_owned(),
            Some(rpc(main[3].as_str())),
        ),
        ask(
            "read_message, not in the window",
            READ_TOKEN,
            "POST",
            "/mcp".to_owned(),
            Some(rpc("404040404040")),
        ),
    ]
}

/// Every case: what was asked, what came back, and every upstream call it took.
async fn record() -> Value {
    let discord = Arc::new(FakeDiscord::new());
    discord.register_channel(&ChannelId(READ_CHANNEL.to_owned()));
    let main = seed(&discord);
    let plain = Arc::new(Plain::new(Arc::clone(&discord)));
    let (state, router) = server(Arc::clone(&plain) as Arc<dyn ChatClient>);
    let mut cases = Vec::new();
    for ask in asks(&main) {
        // Each case from upstream: nothing one case read is answered from memory in the next.
        state.read_ahead.clear();
        let (status, body) = call(&router, ask.token, ask.method, &ask.uri, ask.body.clone()).await;
        cases.push(json!({
            "case": ask.name,
            "request": format!("{} {}", ask.method, ask.uri),
            "status": status.as_u16(),
            "upstream": plain.take(),
            "body": steady(body),
        }));
    }
    Value::Array(cases)
}

#[tokio::test]
async fn a_provider_without_batch_lookups_gets_todays_answers_at_todays_cost() {
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
            "{} no longer answers or reads upstream as it did before batch lookups existed",
            was["case"]
        );
    }
    // The fixture is not vacuous: each kind of caller read upstream, and found what it looked for.
    let upstream = |case: &str| -> Vec<String> {
        held.iter()
            .find(|c| c["case"] == case)
            .and_then(|c| c["upstream"].as_array())
            .expect("a case")
            .iter()
            .map(|line| line.as_str().expect("a line").to_owned())
            .collect()
    };
    let eight = upstream("speech preparation of eight thread messages");
    assert_eq!(eight.len(), 8, "{eight:?}");
    assert!(eight
        .iter()
        .all(|line| line.starts_with("fetch_thread_message")));
    assert_eq!(
        upstream("a thread summary of a row in three parts").len(),
        3,
        "one lookup per part"
    );
    assert_eq!(
        upstream("message by id, in the window"),
        ["fetch_page limit=20 before=- after=-"]
    );
    for case in held {
        if case["case"]
            .as_str()
            .is_some_and(|name| !name.contains(" not "))
        {
            assert_eq!(case["status"], 200, "{case}");
        }
    }
}

/// A provider that looks several messages up by id itself: [`Plain`]'s messages, one
/// `fetch_messages` for any number of ids. An id it does not have is [`Lookup::Absent`]; one named
/// in `unseen` it answers [`Lookup::Unseen`], as a lookup that could not reach it would.
struct Native {
    plain: Plain,
    unseen: Vec<String>,
    /// The read reason each lookup was made under (`remote-ops-audit`, `#287 bulk-reads`).
    reasons: Mutex<Vec<ReadReason>>,
}

impl Native {
    fn new(inner: Arc<FakeDiscord>, unseen: &[&str]) -> Self {
        Self {
            plain: Plain::new(inner),
            unseen: unseen.iter().map(|id| (*id).to_owned()).collect(),
            reasons: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ChatClient for Native {
    fn supports_threading(&self) -> bool {
        true
    }

    fn supports_upstream_read_mark(&self) -> bool {
        true
    }

    fn read_caps(&self, _channel: &ChannelId) -> ReadCaps {
        ReadCaps {
            by_ids: Support::Native,
            ..ReadCaps::default()
        }
    }

    async fn fetch_messages(
        &self,
        channel: &ChannelId,
        thread: Option<&str>,
        ids: &[MessageId],
    ) -> Result<Vec<Lookup>, ChatError> {
        self.reasons
            .lock()
            .expect("log")
            .push(ReadReason::current());
        let held = match thread {
            Some(thread) => self.plain.threads.get(thread).cloned().unwrap_or_default(),
            None => {
                self.plain
                    .inner
                    .fetch_page(channel, 100, None, None)
                    .await?
            }
        };
        Ok(ids
            .iter()
            .map(|id| {
                if self.unseen.iter().any(|unseen| unseen == id.as_str()) {
                    return Lookup::Unseen;
                }
                held.iter()
                    .find(|m| m.id == *id)
                    .map_or(Lookup::Absent, |m| Lookup::Found(Box::new(m.clone())))
            })
            .collect())
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.plain.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.plain.fetch_page(channel, limit, before, after).await
    }

    async fn fetch_thread_message(
        &self,
        channel: &ChannelId,
        thread_id: &str,
        message_id: &MessageId,
    ) -> Result<Message, ChatError> {
        self.plain
            .fetch_thread_message(channel, thread_id, message_id)
            .await
    }

    async fn mark_read_upstream(
        &self,
        channel: &ChannelId,
        through: &MessageId,
    ) -> Result<(), ChatError> {
        self.plain.mark_read_upstream(channel, through).await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.plain.post_message(channel, content, reply_to).await
    }
}

/// A server reading through `provider`, every upstream call it makes counted.
fn counted(provider: Arc<dyn ChatClient>) -> (Arc<CountingChat>, axum::Router) {
    let counting = Arc::new(CountingChat::new(provider));
    let (_state, router) = server(Arc::clone(&counting) as Arc<dyn ChatClient>);
    (counting, router)
}

/// The channel's messages, seeded, and a FakeDiscord holding them.
fn seeded() -> (Arc<FakeDiscord>, Vec<MessageId>) {
    let discord = Arc::new(FakeDiscord::new());
    discord.register_channel(&ChannelId(READ_CHANNEL.to_owned()));
    let main = seed(&discord);
    (discord, main)
}

fn prepared_ids(body: &Value) -> Vec<String> {
    body["prepared"]
        .as_array()
        .expect("prepared")
        .iter()
        .map(|entry| entry["message_id"].as_str().expect("an id").to_owned())
        .collect()
}

/// remote-ops-audit W11's proof: preparing eight thread messages for speech is ONE lookup on a
/// provider that has one, and eight `fetch_thread_message` calls, as today, on one that has not.
#[tokio::test]
async fn preparing_eight_thread_messages_is_one_lookup_where_the_provider_has_one() {
    let eight: Vec<String> = (4..12).map(|n| format!("t{n}")).collect();
    let body = json!({ "ids": eight, "thread_id": THREAD });
    let uri = format!("/api/v1/channels/{READ_CHANNEL}/speech/prepare");

    let (discord, _) = seeded();
    let provider = Arc::new(Native::new(discord, &[]));
    let (native, router) = counted(Arc::clone(&provider) as Arc<dyn ChatClient>);
    let (status, answer) = call(&router, READ_TOKEN, "POST", &uri, Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(prepared_ids(&answer), eight);
    assert_eq!(native.count("fetch_messages"), 1, "{:?}", native.calls());
    assert_eq!(native.count("fetch_thread_message"), 0);
    assert_eq!(native.calls()[0].thread.as_deref(), Some(THREAD));
    assert_eq!(
        *provider.reasons.lock().expect("log"),
        [ReadReason::Speech],
        "counted as speech preparation's, as the reads of one message are (`#287 bulk-reads`)"
    );

    let (discord, _) = seeded();
    let (plain, router) = counted(Arc::new(Plain::new(discord)));
    let (status, answer) = call(&router, READ_TOKEN, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(prepared_ids(&answer), eight, "the same answer");
    assert_eq!(
        plain.count("fetch_thread_message"),
        8,
        "{:?}",
        plain.calls()
    );
    assert_eq!(plain.count("fetch_messages"), 0);
}

/// An id the provider proves absent is left out of a speech preparation, as one outside the window
/// always was, and is a 404 on its own; one it did not see is looked up on its own, as before.
#[tokio::test]
async fn an_absent_message_is_left_out_and_an_unseen_one_is_looked_up_as_before() {
    let (discord, _) = seeded();
    let (native, router) = counted(Arc::new(Native::new(discord, &["t5"])));
    let base = format!("/api/v1/channels/{READ_CHANNEL}");
    let (status, answer) = call(
        &router,
        READ_TOKEN,
        "POST",
        &format!("{base}/speech/prepare"),
        Some(json!({ "ids": ["t4", "t99", "t5"], "thread_id": THREAD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(prepared_ids(&answer), ["t4", "t5"]);
    assert_eq!(native.count("fetch_messages"), 1);
    assert_eq!(
        native.count("fetch_thread_message"),
        1,
        "only the unseen one, on its own: {:?}",
        native.calls()
    );

    let thread = THREAD.replace('/', "%2F");
    let (status, answer) = call(
        &router,
        READ_TOKEN,
        "GET",
        &format!("{base}/messages/t99?thread_id={thread}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["error"], "unknown_message");
    let (status, answer) = call(
        &router,
        READ_TOKEN,
        "GET",
        &format!("{base}/messages/t5?thread_id={thread}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["message"]["content"], "reply 5");
    assert_eq!(
        native.count("fetch_thread_message"),
        2,
        "{:?}",
        native.calls()
    );
}

/// Every caller of a lookup by id reads through one lookup on a provider that has one: the
/// message route (in a thread and on the channel's own messages, where it finds a message older
/// than the window), a thread summary of a row in three parts, the upstream read mark's check,
/// and MCP `read_message`. None of them reads a window or looks a message up on its own.
#[tokio::test]
async fn every_caller_reads_through_one_lookup() {
    let (discord, main) = seeded();
    // Twenty more, so the first message of the channel is older than the window.
    for i in 0..20 {
        discord.seed_at(
            &ChannelId(READ_CHANNEL.to_owned()),
            "ada",
            &format!("later {i}"),
            BASE_MS + 60 * MINUTE_MS + i * MINUTE_MS,
        );
    }
    let (native, router) = counted(Arc::new(Native::new(discord, &[])));
    let base = format!("/api/v1/channels/{READ_CHANNEL}");
    let thread = THREAD.replace('/', "%2F");
    let lookups = || native.count("fetch_messages");
    let others = || native.calls().len() - native.count("fetch_messages");

    let (status, answer) = call(
        &router,
        READ_TOKEN,
        "GET",
        &format!("{base}/messages/{}", main[0].as_str()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["message"]["content"], "main message 0");
    assert!(
        !answer["message"]["spoken_time"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "prepared as every read is: {answer}"
    );
    assert_eq!((lookups(), others()), (1, 0), "{:?}", native.calls());

    let (status, answer) = call(
        &router,
        READ_TOKEN,
        "GET",
        &format!("{base}/messages/t7?thread_id={thread}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!((lookups(), others()), (2, 0));

    let (status, answer) = call(
        &router,
        WRITE_TOKEN,
        "GET",
        &format!("{base}/messages/t1/summary?thread_id={thread}&with=t2,t3"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["state"], "generated", "{answer}");
    assert_eq!((lookups(), others()), (3, 0), "three parts, one lookup");

    let (status, answer) = call(
        &router,
        WRITE_TOKEN,
        "POST",
        &format!("{base}/upstream-read"),
        Some(json!({ "message_id": "t6", "thread_id": THREAD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(lookups(), 4);
    assert_eq!(native.count("mark_read_upstream"), 1);
    let (status, _) = call(
        &router,
        WRITE_TOKEN,
        "POST",
        &format!("{base}/upstream-read"),
        Some(json!({ "message_id": "t99", "thread_id": THREAD })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an absent message is never marked"
    );
    assert_eq!(native.count("mark_read_upstream"), 1);

    let (status, answer) = call(
        &router,
        READ_TOKEN,
        "POST",
        "/mcp",
        Some(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "read_message",
                "arguments": { "channel_id": READ_CHANNEL, "message_id": main[2].as_str() },
            },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(
        answer["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("main message 2")),
        "{answer}"
    );
    assert_eq!(lookups(), 6);
    assert_eq!(
        others(),
        1,
        "nothing but the one mark: {:?}",
        native.calls()
    );
}

/// A message of the channel's own the lookup did not see is found in the recent window, as before;
/// and a channel outside the allowlist is refused before anything is asked of the provider.
#[tokio::test]
async fn an_unseen_message_of_the_channel_is_found_in_the_window_and_the_allowlist_comes_first() {
    let (discord, main) = seeded();
    let unseen = main[4].as_str().to_owned();
    let (native, router) = counted(Arc::new(Native::new(discord, &[unseen.as_str()])));
    let (status, answer) = call(
        &router,
        READ_TOKEN,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/messages/{unseen}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["message"]["content"], "main message 4");
    assert_eq!(native.count("fetch_messages"), 1);
    assert_eq!(
        native.count("fetch_recent_dated"),
        1,
        "the window: {:?}",
        native.calls()
    );

    let before = native.calls().len();
    for uri in [
        "/api/v1/channels/999999/messages/1".to_owned(),
        format!("/api/v1/channels/999999/messages/1?thread_id={THREAD}"),
    ] {
        let (status, _) = call(&router, READ_TOKEN, "GET", &uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    assert_eq!(native.calls().len(), before, "nothing was asked upstream");
}

// ---- Slack: a run of timestamps a call --------------------------------------------------------

const SLACK_CH: &str = "C0000MAIN1";

fn slack_channel() -> ChannelId {
    ChannelId(SLACK_CH.to_owned())
}

fn slack_id(ts: &str) -> MessageId {
    vibe_talk::slack::message_id_from_ts(ts).expect("encodable")
}

/// Sixty messages a second apart, a system event at second 70, and a thread on the first message
/// with thirty replies a second apart from second 100.
fn slack_state() -> FakeState {
    let mut state = FakeState::default();
    let mut list: Vec<Value> = (0..60)
        .map(|i| profiled(&ts(i, 0), "UADA00001", "Ada", &format!("main {i}")))
        .collect();
    list.push(system(&ts(70, 0), "channel_join"));
    let replies: Vec<(String, String)> = (0..30)
        .map(|i| (ts(100 + i, 0), format!("reply {i}")))
        .collect();
    let mut root = list[0].clone();
    slack_fake::thread(&mut state, SLACK_CH, &mut root, &replies);
    list[0] = root;
    state.history.insert(SLACK_CH.to_owned(), list);
    state
}

fn contents(answers: &[Lookup]) -> Vec<String> {
    answers
        .iter()
        .map(|answer| match answer {
            Lookup::Found(message) => message.content.clone(),
            Lookup::Absent => "<absent>".to_owned(),
            Lookup::Unseen => "<unseen>".to_owned(),
        })
        .collect()
}

/// remote-ops-audit W11, Slack: eight replies of a thread are ONE `conversations.replies` call,
/// pinned a microsecond either side of the first and last, where one lookup each was eight.
#[tokio::test]
async fn slack_looks_up_eight_replies_of_a_thread_in_one_call() {
    let fake = slack_fake::Fake::start(slack_state(), 20).await;
    let client = fake.client();
    let root = ts(0, 0);
    let asked: Vec<MessageId> = (3..11).map(|i| slack_id(&ts(100 + i, 0))).collect();
    assert_eq!(client.read_caps(&slack_channel()).by_ids, Support::Native);
    let answers = client
        .fetch_messages(&slack_channel(), Some(&root), &asked)
        .await
        .expect("found");
    assert_eq!(
        contents(&answers),
        (3..11).map(|i| format!("reply {i}")).collect::<Vec<_>>()
    );
    let calls = fake.requests("conversations.replies");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["ts"], root);
    assert_eq!(calls[0]["oldest"], ts(102, 999_999));
    assert_eq!(calls[0]["latest"], ts(110, 1));
    assert_eq!(calls[0]["inclusive"], "true");
    assert_eq!(fake.calls("conversations.history"), 0);

    // What the same eight cost one at a time, as every caller looked them up before.
    for id in &asked {
        client
            .fetch_thread_message(&slack_channel(), &root, id)
            .await
            .expect("found");
    }
    assert_eq!(fake.calls("conversations.replies"), 1 + 8);
}

/// Ids far apart are a call per run, each id proved absent only by a page that covered it, and a
/// message this server would not show among the channel's own is unseen rather than absent.
#[tokio::test]
async fn slack_reads_ids_far_apart_a_run_a_call_and_proves_absence_only_where_it_read() {
    let fake = slack_fake::Fake::start(slack_state(), 20).await;
    let client = fake.client();
    let asked = [
        slack_id(&ts(5, 0)),
        slack_id(&ts(7, 500_000)), // between two messages: none there
        slack_id(&ts(50, 0)),
        slack_id(&ts(70, 0)),  // the system event
        slack_id(&ts(105, 0)), // a reply: not one of the channel's own messages
    ];
    let answers = client
        .fetch_messages(&slack_channel(), None, &asked)
        .await
        .expect("read");
    assert_eq!(
        contents(&answers),
        ["main 5", "<absent>", "main 50", "<unseen>", "<absent>"]
    );
    let calls = fake.requests("conversations.history");
    // [5, 105]: the twenty newest are the reply-free stretch 70 down to 41, proving 105 absent
    // and finding 70 and 50; then [5, 7.5] in one page, finding 5 and proving 7.5 absent.
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0]["oldest"], ts(4, 999_999));
    assert_eq!(calls[0]["latest"], ts(105, 1));
    assert_eq!(calls[1]["oldest"], ts(4, 999_999));
    assert_eq!(calls[1]["latest"], ts(7, 500_001));
    assert_eq!(fake.calls("conversations.replies"), 0);
}

/// A Slack deployment built from configuration exactly as `main` builds one, its one channel the
/// conversation the Slack fixture fills.
fn slack_deployment(fake: &slack_fake::Fake) -> AppState {
    let text = format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{base}"
token = "{token}"

[[channels]]
id = "{SLACK_CH}"
label = "team"
writable = false
"#,
        base = fake.base,
        token = slack_fake::TOKEN,
    );
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

/// Through the routes: a message by id on Slack is one pinned read wherever it is, and a deleted
/// one is a 404 from that same read, where both were a read of the recent window before.
#[tokio::test]
async fn slack_answers_a_message_by_id_with_one_pinned_read() {
    let fake = slack_fake::Fake::start(slack_state(), 20).await;
    let router = vibe_talk::http::router(slack_deployment(&fake));
    let base = format!("/api/v1/channels/{SLACK_CH}");
    let old = slack_id(&ts(2, 0));
    let (status, answer) = call(
        &router,
        READ_TOKEN,
        "GET",
        &format!("{base}/messages/{}", old.as_str()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["message"]["content"], "main 2",
        "older than the window"
    );
    let calls = fake.requests("conversations.history");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["latest"], ts(2, 1));

    fake.with(|state| {
        state
            .history
            .get_mut(SLACK_CH)
            .expect("channel")
            .retain(|message| message["ts"] != json!(ts(3, 0)));
    });
    let (status, answer) = call(
        &router,
        READ_TOKEN,
        "GET",
        &format!("{base}/messages/{}", slack_id(&ts(3, 0)).as_str()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["error"], "unknown_message");
    assert_eq!(fake.calls("conversations.history"), 2, "no window was read");
}

// ---- A bridge: every id in one request, only when configured to --------------------------------

/// A loopback bridge holding [`thread`]'s messages, which answers a lookup by id and a lookup of
/// one thread message, and records every request it is sent.
async fn lookup_bridge() -> (String, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = Arc::clone(&seen);
    let wire = |message: &Message| {
        json!({
            "id": message.id.as_str(),
            "channel_id": READ_CHANNEL,
            "content": message.content,
            "timestamp": message.timestamp,
            "author": { "id": "7", "username": "grace", "bot": false },
            "thread": { "id": THREAD, "root_message_id": "t0", "is_root": message.id.as_str() == "t0",
                        "reply_count": null, "reply_count_exact": false },
        })
    };
    let held: Vec<Value> = thread().iter().map(wire).collect();
    let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
        let log = Arc::clone(&log);
        let held = held.clone();
        async move {
            log.lock().expect("log").push(uri.to_string());
            let id_of = |value: &Value| value["id"].as_str().unwrap_or_default().to_owned();
            if let Some(query) = uri.query().filter(|query| query.contains("ids=")) {
                let asked: Vec<String> = query
                    .split('&')
                    .filter_map(|pair| pair.strip_prefix("ids="))
                    .map(str::to_owned)
                    .collect();
                let found: Vec<Value> = held
                    .iter()
                    .filter(|m| asked.contains(&id_of(m)))
                    .cloned()
                    .collect();
                let missing: Vec<&String> = asked
                    .iter()
                    .filter(|id| !found.iter().any(|m| id_of(m) == **id))
                    .collect();
                return axum::Json(json!({ "messages": found, "missing": missing }))
                    .into_response();
            }
            let id = uri.path().rsplit('/').next().unwrap_or_default().to_owned();
            match held.iter().find(|m| id_of(m) == id) {
                Some(message) => axum::Json(message.clone()).into_response(),
                None => (StatusCode::NOT_FOUND, "no such message").into_response(),
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (base, seen)
}

fn bridge_deployment(base: &str, extra: &str) -> AppState {
    let text = vibe_talk::testing::config_toml().replace(
        "[discord]",
        &format!("[discord]\nthread_api = \"bridge\"\napi_base = \"{base}\"\n{extra}"),
    );
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

/// remote-ops-audit W11 `#287 bulk-reads`: a bridge configured with `id_reads` is asked for eight
/// thread messages in one request; without the setting, the same preparation is the eight lookups
/// of one message it always was, and the bridge is never sent the request it may not serve.
#[tokio::test]
async fn a_bridge_looks_up_eight_thread_messages_in_one_request_only_when_configured_to() {
    let eight: Vec<String> = (4..12).map(|n| format!("t{n}")).collect();
    let body = json!({ "ids": eight, "thread_id": THREAD });
    let uri = format!("/api/v1/channels/{READ_CHANNEL}/speech/prepare");

    let (base, seen) = lookup_bridge().await;
    let router = vibe_talk::http::router(bridge_deployment(&base, "id_reads = true"));
    let (status, answer) = call(&router, READ_TOKEN, "POST", &uri, Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(prepared_ids(&answer), eight);
    let asked = seen.lock().expect("log").clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(
        asked[0],
        format!(
            "/channels/{READ_CHANNEL}/messages?{}&thread={}",
            eight
                .iter()
                .map(|id| format!("ids={id}"))
                .collect::<Vec<_>>()
                .join("&"),
            THREAD.replace('/', "%2F")
        )
    );

    let (base, seen) = lookup_bridge().await;
    let router = vibe_talk::http::router(bridge_deployment(&base, ""));
    let (status, answer) = call(&router, READ_TOKEN, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(prepared_ids(&answer), eight, "the same answer");
    let asked = seen.lock().expect("log").clone();
    assert_eq!(asked.len(), 8, "{asked:?}");
    assert!(
        asked.iter().all(|uri| uri.starts_with(&format!(
            "/channels/{READ_CHANNEL}/threads/{}/messages/t",
            THREAD.replace('/', "%2F")
        ))),
        "{asked:?}"
    );
}
