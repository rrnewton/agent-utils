//! remote-ops-audit W13 `#287 bulk-reads`: the newest messages of several channels in one call.
//!
//! The first test pins what a voice catch-up (`/activity`) and a call start's warm-up read
//! upstream of five channels, and what they answer, on a provider that overrides nothing but the
//! methods every provider must write: cold, at once again, after the reuse period with an edit, a
//! delete and a new message in between, a warm-up and the catch-up after it, a catch-up while one
//! channel's read fails, and one after the page has just read a channel. Its fixture was recorded
//! by the code as it stood BEFORE `ChatClient::fetch_recent_many` existed, so it is the proof that
//! a provider without a read of several channels of its own still gets those answers byte for
//! byte, at the same cost, in the same order, once the callers know about it. Rewrite it only for
//! a change that is MEANT to alter what such a provider is asked or answers:
//! `VIBE_TALK_UPDATE_FIXTURES=1 cargo test --test recent_many`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{
    ChatClient, ChatError, ChatIdentity, ReadCaps, RecentAnswers, RecentAsk, RecentRead, Support,
};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, ChannelInfo, Message, MessageId};
use vibe_talk::providers::{ChatRouter, ProviderEntry};
use vibe_talk::state::AppState;
use vibe_talk::testing::{CountingChat, READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL};

/// The three channels past the two the test configuration has, so a catch-up reads five.
const EXTRA: [&str; 3] = ["3333333333", "4444444444", "5555555555"];
const MINUTE_MS: i64 = 60_000;
/// What each of the five channels says first, in the order the server lists them.
const FIRST_WORDS: [&str; 5] = [
    "build is green",
    "ship it",
    "pager is quiet",
    "tests are flaky",
    "lunch at noon",
];

fn channel(id: &str) -> ChannelId {
    ChannelId(id.to_owned())
}

/// Every channel of the server, in the order it lists them.
fn five() -> Vec<&'static str> {
    let mut all = vec![READ_CHANNEL, WRITE_CHANNEL];
    all.extend(EXTRA);
    all
}

/// A provider that writes only what the trait requires and logs every call that may reach
/// upstream. Everything else runs the trait's default body on THIS client, which is the case the
/// fixture pins: a provider with no read of several channels of its own.
struct Plain {
    inner: Arc<FakeDiscord>,
    log: Mutex<Vec<String>>,
    /// Channels whose reads fail, as a provider's do when it cannot reach one.
    failing: Mutex<HashSet<String>>,
}

impl Plain {
    fn note(&self, line: String) {
        self.log.lock().expect("log").push(line);
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().expect("log"))
    }

    fn fail(&self, id: &str, failing: bool) {
        let mut set = self.failing.lock().expect("failing");
        if failing {
            set.insert(id.to_owned());
        } else {
            set.remove(id);
        }
    }

    fn refuse(&self, channel: &ChannelId) -> Result<(), ChatError> {
        if self
            .failing
            .lock()
            .expect("failing")
            .contains(channel.as_str())
        {
            return Err(ChatError::Transport(
                "the channel is unreachable".to_owned(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl ChatClient for Plain {
    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.note("identity".to_owned());
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
            "fetch_page {channel} limit={limit} before={} after={}",
            before.map_or("-", MessageId::as_str),
            after.map_or("-", MessageId::as_str),
        ));
        self.refuse(channel)?;
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        self.note(format!("fetch_recent {channel} limit={limit}"));
        self.refuse(channel)?;
        self.inner.fetch_recent(channel, limit).await
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

/// Every message id this run minted, and the label the fixture knows it by: ids carry the wall
/// clock's time, so the same message has another id on every run.
#[derive(Default)]
struct Labels(Vec<(String, String)>);

impl Labels {
    fn add(&mut self, id: &MessageId, label: &str) {
        self.0.push((id.as_str().to_owned(), format!("<{label}>")));
    }

    fn text(&self, text: &str) -> String {
        self.0
            .iter()
            .fold(text.to_owned(), |text, (id, label)| text.replace(id, label))
    }

    fn lines(&self, lines: Vec<String>) -> Value {
        lines.iter().map(|line| self.text(line)).collect()
    }

    /// An answer with every id labelled, and every instant that follows the wall clock replaced
    /// by a fixed word.
    fn steady(&self, value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(&text)),
            Value::Array(items) => items.into_iter().map(|item| self.steady(item)).collect(),
            Value::Object(fields) => fields
                .into_iter()
                .map(|(key, value)| {
                    let value = match key.as_str() {
                        "timestamp" | "spoken_time" | "since" if value.is_string() => {
                            json!(format!("<{key}>"))
                        }
                        _ => self.steady(value),
                    };
                    (key, value)
                })
                .collect(),
            other => other,
        }
    }
}

/// A server with five channels, read through `plain`.
fn server() -> (AppState, Arc<FakeDiscord>, Arc<Plain>) {
    let extra: String = EXTRA
        .iter()
        .map(|id| {
            format!("[[channels]]\nid = \"{id}\"\nlabel = \"extra {id}\"\nwritable = false\n\n")
        })
        .collect();
    let toml =
        vibe_talk::testing::config_toml().replace("[elevenlabs]", &format!("{extra}[elevenlabs]"));
    let (mut state, fake, _elevenlabs) = vibe_talk::testing::state_from_toml(&toml);
    for id in EXTRA {
        fake.register_channel(&channel(id));
    }
    let plain = Arc::new(Plain {
        inner: Arc::clone(&fake),
        log: Mutex::new(Vec::new()),
        failing: Mutex::new(HashSet::new()),
    });
    state.replace_chat(Arc::clone(&plain) as Arc<dyn ChatClient>);
    (state, fake, plain)
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {READ_TOKEN}"))
        .body(Body::empty())
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

/// A voice catch-up, as `recent_activity` asks for one, projected onto what it answered channel by
/// channel: what it said, how it counted, whether it was current or a held copy (and how old, and
/// why), and the newest message it saw.
async fn catch_up(state: &AppState) -> Value {
    let activity = vibe_talk::ops::activity(
        state,
        vibe_talk::ops::ActivityRequest {
            hours: None,
            per_channel_limit: None,
            now_ms: vibe_talk::ops::now_ms(),
        },
    )
    .await;
    let activity = match activity {
        Ok(activity) => activity,
        Err(error) => return json!({ "error": error.code() }),
    };
    let newest = activity
        .newest()
        .map(|(channel, newest)| json!([channel.id.as_str(), newest.entry.summary]));
    let channels: Vec<Value> = activity
        .channels
        .iter()
        .map(|part| {
            let currency = match part.currency {
                vibe_talk::ops::Currency::Current => json!("current"),
                vibe_talk::ops::Currency::Held { age, failed } => {
                    json!({ "held_seconds": age.as_secs(), "failed": failed })
                }
            };
            let read = match &part.read {
                Ok(read) => json!({
                    "said": read.entries.iter().map(|e| e.summary.clone()).collect::<Vec<_>>(),
                    "counted": read.counted,
                    "complete": read.complete,
                    "noise": read.noise,
                }),
                Err(error) => json!({ "error": error.code() }),
            };
            json!({
                "id": part.channel.id.as_str(),
                "read": read,
                "currency": currency,
                "as_of": part.as_of,
            })
        })
        .collect();
    json!({ "channels": channels, "newest": newest })
}

/// Every case, in order on one server: what was asked, what came back, and every upstream call.
#[allow(clippy::too_many_lines)]
async fn record() -> Value {
    let (state, fake, plain) = server();
    let router = vibe_talk::http::router(state.clone());
    let mut labels = Labels::default();
    let now = vibe_talk::ops::now_ms();
    let mut seeded = Vec::new();
    for (n, (id, text)) in five().into_iter().zip(FIRST_WORDS).enumerate() {
        let n = i64::try_from(n).expect("small");
        let label = format!("c{n}-1");
        let message = fake.seed_at(
            &channel(id),
            "agent",
            text,
            now - 60 * MINUTE_MS + n * 1_000,
        );
        labels.add(&message, &label);
        seeded.push(message);
    }
    let mut cases = Vec::new();
    let mut case = |name: &str, upstream: Vec<String>, answer: Value, labels: &Labels| {
        cases.push(json!({
            "case": name,
            "upstream": labels.lines(upstream),
            "answer": labels.steady(answer),
        }));
    };

    let answer = catch_up(&state).await;
    case(
        "a catch-up of five channels with nothing held",
        plain.take(),
        answer,
        &labels,
    );
    let answer = catch_up(&state).await;
    case("a catch-up at once after it", plain.take(), answer, &labels);

    fake.edit(&seeded[1], "ship it tomorrow, edited");
    fake.delete(&seeded[2]);
    let arrived = fake.seed_at(
        &channel(EXTRA[2]),
        "grace",
        "something new",
        now - 10 * MINUTE_MS,
    );
    labels.add(&arrived, "c4-2");
    tokio::time::sleep(Duration::from_secs(61)).await;
    let answer = catch_up(&state).await;
    case(
        "a catch-up after the reuse period, after an edit, a delete and a new message",
        plain.take(),
        answer,
        &labels,
    );

    tokio::time::sleep(Duration::from_secs(61)).await;
    let warming = vibe_talk::recent::warm(&state);
    tokio::time::sleep(Duration::from_millis(100)).await;
    case(
        "a call start's warm-up after the reuse period",
        plain.take(),
        json!({ "warming": warming }),
        &labels,
    );
    let answer = catch_up(&state).await;
    case(
        "a catch-up right after the warm-up",
        plain.take(),
        answer,
        &labels,
    );

    plain.fail(EXTRA[1], true);
    tokio::time::sleep(Duration::from_secs(61)).await;
    let answer = catch_up(&state).await;
    case(
        "a catch-up while one channel's read fails",
        plain.take(),
        answer,
        &labels,
    );
    plain.fail(EXTRA[1], false);

    tokio::time::sleep(Duration::from_secs(61)).await;
    let (status, page) = get(
        &router,
        &format!("/api/v1/channels/{WRITE_CHANNEL}/messages"),
    )
    .await;
    let said: Vec<Value> = page["messages"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|m| m["content"].clone())
        .collect();
    let answer = catch_up(&state).await;
    case(
        "the page reads one channel, then a catch-up",
        plain.take(),
        json!({
            "page": { "status": status.as_u16(), "said": said },
            "activity": answer,
        }),
        &labels,
    );
    Value::Array(cases)
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recent_many_default.json")
}

#[tokio::test(start_paused = true)]
async fn a_provider_without_a_read_of_several_channels_gets_todays_answers_at_todays_cost() {
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
            "{} no longer answers or reads upstream as it did before reads of several channels \
             existed",
            was["case"]
        );
    }
    // The fixture is not vacuous: every catch-up that had to read did, one read per channel, and
    // the reads saw what changed.
    let case = |name: &str| -> &Value {
        held.iter()
            .find(|c| c["case"] == name)
            .unwrap_or_else(|| panic!("no case {name}"))
    };
    let upstream = |name: &str| -> Vec<String> {
        case(name)["upstream"]
            .as_array()
            .expect("lines")
            .iter()
            .map(|line| line.as_str().expect("a line").to_owned())
            .collect()
    };
    let cold = upstream("a catch-up of five channels with nothing held");
    assert_eq!(cold.len(), 5, "{cold:?}");
    assert!(cold.iter().all(|line| line.starts_with("fetch_recent ")));
    assert!(upstream("a catch-up at once after it").is_empty());
    let changed = "a catch-up after the reuse period, after an edit, a delete and a new message";
    assert_eq!(upstream(changed).len(), 5);
    let text = case(changed)["answer"].to_string();
    assert!(text.contains("ship it tomorrow, edited"), "{text}");
    assert!(text.contains("something new"), "{text}");
    assert!(
        !text.contains(FIRST_WORDS[2]),
        "the deleted message is gone: {text}"
    );
    assert_eq!(
        case("a call start's warm-up after the reuse period")["answer"]["warming"],
        5
    );
    assert_eq!(
        upstream("a call start's warm-up after the reuse period").len(),
        5
    );
    assert!(upstream("a catch-up right after the warm-up").is_empty());
    assert_eq!(
        upstream("a catch-up while one channel's read fails").len(),
        5
    );
    let failed = case("a catch-up while one channel's read fails")["answer"].to_string();
    assert!(failed.contains("chat_error"), "{failed}");
    let page_then = upstream("the page reads one channel, then a catch-up");
    assert_eq!(
        page_then.len(),
        5,
        "one page read, then the other four: {page_then:?}"
    );
    assert!(page_then[0].contains(WRITE_CHANNEL));
}

/// A provider that reads several channels together itself (or says it does not, with
/// `native: false`), answering each ask from its fake's newest messages and logging the channels
/// of each call. What it reads to answer is not an upstream call of its own: a [`CountingChat`]
/// around it counts what the server asks.
struct Gathered {
    inner: Arc<FakeDiscord>,
    native: bool,
    /// What every answer says about its currency; see [`RecentRead::as_of`].
    as_of: Mutex<Option<String>>,
    /// Channels the provider leaves out of its answers.
    leaves_out: Mutex<HashSet<String>>,
    asked: Mutex<Vec<Vec<String>>>,
}

impl Gathered {
    fn new(inner: Arc<FakeDiscord>, native: bool) -> Self {
        Self {
            inner,
            native,
            as_of: Mutex::new(None),
            leaves_out: Mutex::new(HashSet::new()),
            asked: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> Vec<Vec<String>> {
        self.asked.lock().expect("log").clone()
    }
}

#[async_trait]
impl ChatClient for Gathered {
    fn read_caps(&self, _channel: &ChannelId) -> ReadCaps {
        ReadCaps {
            many_channels: if self.native {
                Support::Native
            } else {
                Support::Emulated
            },
            ..ReadCaps::default()
        }
    }

    async fn fetch_recent_many(&self, asks: &[(ChannelId, RecentAsk)]) -> RecentAnswers {
        self.asked
            .lock()
            .expect("log")
            .push(asks.iter().map(|(c, _)| c.0.clone()).collect());
        let as_of = self.as_of.lock().expect("setting").clone();
        let leaves_out = self.leaves_out.lock().expect("setting").clone();
        let mut answers = Vec::new();
        for (channel, ask) in asks {
            let read = if leaves_out.contains(channel.as_str()) {
                Err(ChatError::Refused("left out".to_owned()))
            } else {
                self.inner
                    .fetch_recent(channel, ask.limit)
                    .await
                    .map(|messages| RecentRead {
                        messages,
                        as_of: as_of.clone(),
                        exhausted: false,
                    })
            };
            answers.push((channel.clone(), read));
        }
        answers
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
        self.inner.fetch_page(channel, limit, before, after).await
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

fn routed(id: &str, provider: &str) -> ChannelInfo {
    ChannelInfo {
        id: channel(id),
        label: id.to_owned(),
        writable: false,
        alias: None,
        added: false,
        provider: Some(provider.to_owned()),
    }
}

/// remote-ops-audit W13, the router's proof: five channels on a provider that reads several
/// together and one on a provider that does not are one call to the first, for all five of its
/// channels, and the one read the second's default is made of; every answer comes back in the
/// order asked, and a channel no provider holds is refused in its own place and asked of nobody.
#[tokio::test]
async fn the_router_asks_each_provider_once_for_its_own_channels() {
    let fake_a = Arc::new(FakeDiscord::new());
    let fake_b = Arc::new(FakeDiscord::new());
    let a_ids = ["A1", "A2", "A3", "A4", "A5"];
    for id in a_ids {
        fake_a.seed(&channel(id), "ada", &format!("said in {id}"));
    }
    fake_b.seed(&channel("B1"), "ada", "said in B1");
    let gathered_a = Arc::new(Gathered::new(fake_a, true));
    let gathered_b = Arc::new(Gathered::new(fake_b, false));
    let counting_a = Arc::new(CountingChat::new(
        Arc::clone(&gathered_a) as Arc<dyn ChatClient>
    ));
    let counting_b = Arc::new(CountingChat::new(
        Arc::clone(&gathered_b) as Arc<dyn ChatClient>
    ));
    let entry = |key: &str, client: Arc<CountingChat>| ProviderEntry {
        key: key.to_owned(),
        namespace: key.to_owned(),
        client,
        live_poll_seconds: 0,
    };
    let router = ChatRouter::new(
        vec![
            entry("a", Arc::clone(&counting_a)),
            entry("b", Arc::clone(&counting_b)),
        ],
        None,
    );
    let mut routes: Vec<ChannelInfo> = a_ids.iter().map(|id| routed(id, "a")).collect();
    routes.push(routed("B1", "b"));
    router.replace_routes(&routes);

    let asked: Vec<ChannelId> = ["A1", "A2", "B1", "A3", "NOWHERE", "A4", "A5"]
        .iter()
        .map(|id| channel(id))
        .collect();
    let asks: Vec<(ChannelId, RecentAsk)> = asked
        .iter()
        .map(|channel| (channel.clone(), RecentAsk::newest(20)))
        .collect();
    let answers = router.fetch_recent_many(&asks).await;

    assert_eq!(counting_a.count("fetch_recent_many"), 1);
    assert_eq!(
        counting_a.calls().len(),
        1,
        "one call for all five, and no read of any one channel"
    );
    assert_eq!(gathered_a.asked(), [a_ids.to_vec()]);
    assert_eq!(counting_b.count("fetch_recent_many"), 0);
    assert_eq!(
        counting_b.count("fetch_recent_dated"),
        1,
        "the emulated provider is read as it always was"
    );
    assert!(
        gathered_b.asked().is_empty(),
        "an emulated provider's own read of several channels is never asked"
    );
    assert_eq!(
        answers.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
        asked
    );
    for (at, (id, (_, answer))) in asked.iter().zip(&answers).enumerate() {
        if id.as_str() == "NOWHERE" {
            assert!(
                matches!(answer, Err(ChatError::Refused(_))),
                "{at}: {answer:?}"
            );
            continue;
        }
        let read = answer.as_ref().expect("a read");
        assert_eq!(
            read.messages
                .iter()
                .map(|m| m.content.clone())
                .collect::<Vec<_>>(),
            [format!("said in {id}")],
            "{at}: each channel's own messages, in its own place"
        );
    }
}
