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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{READ_CHANNEL, READ_TOKEN, WRITE_TOKEN};
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
