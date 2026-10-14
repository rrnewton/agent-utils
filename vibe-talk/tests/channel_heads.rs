//! remote-ops-audit W12 `#287 bulk-reads`: the newest message of several channels in one call.
//!
//! The first test pins what every caller that only needs to know whether a channel moved reads
//! upstream, and what it answers, on a provider that overrides nothing but the methods every
//! provider must write: the live poller's ticks, a voice catch-up (`/activity`) cold, reused and
//! after its reuse period, with an edit and a delete in between, a call start's warm-up, the
//! startup probe and diagnostics. Its fixture was recorded by the code as it stood BEFORE
//! `ChatClient::channel_heads` existed, so it is the proof that a provider without a head read of
//! its own still gets those answers byte for byte, at the same cost, once the callers know about
//! heads. Rewrite it only for a change that is MEANT to alter what such a provider is asked or
//! answers: `VIBE_TALK_UPDATE_FIXTURES=1 cargo test --test channel_heads`.

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
    ChannelHead, ChatClient, ChatError, ChatIdentity, HeadAnswers, ReadCaps, Support,
};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, ChannelInfo, Message, MessageId};
use vibe_talk::providers::{ChatRouter, ProviderEntry};
use vibe_talk::state::AppState;
use vibe_talk::testing::{CountingChat, READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL};

/// A third channel, so every caller here reads more than two.
const THIRD: &str = "3333333333";
/// The live poll interval, the floor the configuration allows.
const POLL: Duration = Duration::from_secs(5);
const MINUTE_MS: i64 = 60_000;

fn channel(id: &str) -> ChannelId {
    ChannelId(id.to_owned())
}

/// A provider that writes only what the trait requires and logs every call that may reach
/// upstream. Everything else runs the trait's default body on THIS client, which is the case the
/// fixture pins: a provider with no head read of its own.
struct Plain {
    inner: Arc<FakeDiscord>,
    log: Mutex<Vec<String>>,
}

impl Plain {
    fn note(&self, line: String) {
        self.log.lock().expect("log").push(line);
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().expect("log"))
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
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        self.note(format!("fetch_recent {channel} limit={limit}"));
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

/// A server with three channels, polled every [`POLL`], reading through `plain`.
fn server() -> (AppState, Arc<FakeDiscord>, Arc<Plain>) {
    let toml = vibe_talk::testing::config_toml()
        .replace(
            "max_fetch_limit = 50",
            &format!(
                "max_fetch_limit = 50\nlive_poll_seconds = {}",
                POLL.as_secs()
            ),
        )
        .replace(
            "[elevenlabs]",
            &format!(
                "[[channels]]\nid = \"{THIRD}\"\nlabel = \"night shift\"\nwritable = false\n\n\
                 [elevenlabs]"
            ),
        );
    let (mut state, fake, _elevenlabs) = vibe_talk::testing::state_from_toml(&toml);
    fake.register_channel(&channel(THIRD));
    let plain = Arc::new(Plain {
        inner: Arc::clone(&fake),
        log: Mutex::new(Vec::new()),
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

/// What a catch-up answered, channel by channel: what it said, how it counted, and whether it
/// was a held copy.
fn activity(status: StatusCode, body: &Value) -> Value {
    let channels: Vec<Value> = body["channels"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|part| {
            let said: Vec<&Value> = part["entries"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|entry| &entry["summary"])
                .collect();
            json!({
                "id": part["channel"]["id"],
                "error": part["error"],
                "said": said,
                "counted": part["counted"],
                "at_least": part["at_least"],
                "held_age_seconds": part["held_age_seconds"],
            })
        })
        .collect();
    json!({
        "status": status.as_u16(),
        "channels": channels,
        "newest": body["newest"]["entry"]["summary"],
    })
}

/// What the poller has published in each channel so far, oldest first.
fn published(state: &AppState) -> Value {
    [READ_CHANNEL, WRITE_CHANNEL, THIRD]
        .into_iter()
        .map(|id| {
            let contents: Vec<String> = state
                .live
                .subscribe(&channel(id), None)
                .replay
                .into_iter()
                .map(|live| live.message.content)
                .collect();
            (id.to_owned(), json!(contents))
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// Every case, in order on one server: what was asked, what came back, and every upstream call.
#[allow(clippy::too_many_lines)]
async fn record() -> Value {
    let (state, fake, plain) = server();
    let router = vibe_talk::http::router(state.clone());
    let mut labels = Labels::default();
    let now = vibe_talk::ops::now_ms();
    for (second, (id, text, label)) in [
        (READ_CHANNEL, "build is green", "read-1"),
        (WRITE_CHANNEL, "ship it", "write-1"),
        (THIRD, "pager is quiet", "third-1"),
    ]
    .into_iter()
    .enumerate()
    {
        let at = now - 60 * MINUTE_MS + i64::try_from(second).expect("small") * 1_000;
        let seeded = fake.seed_at(&channel(id), "agent", text, at);
        labels.add(&seeded, label);
    }
    let mut cases = Vec::new();
    let mut case = |name: &str, upstream: Vec<String>, answer: Value, labels: &Labels| {
        cases.push(json!({
            "case": name,
            "upstream": labels.lines(upstream),
            "answer": labels.steady(answer),
        }));
    };

    // The live poller, as `main` starts it for a polled provider: its own client, every channel.
    let client = Arc::clone(&state.providers.entries()[0].client);
    let poller = tokio::spawn(vibe_talk::live::poll_forever(
        state.clone(),
        None,
        client,
        state.config.chat.default_fetch_limit,
        POLL,
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    case(
        "the poller's first tick seeds every channel",
        plain.take(),
        published(&state),
        &labels,
    );
    let arrived = fake.seed_at(
        &channel(READ_CHANNEL),
        "grace",
        "deploy finished",
        now - 30 * MINUTE_MS,
    );
    labels.add(&arrived, "read-2");
    tokio::time::sleep(POLL).await;
    case(
        "a tick after a message arrived in one channel",
        plain.take(),
        published(&state),
        &labels,
    );
    tokio::time::sleep(POLL).await;
    case("a quiet tick", plain.take(), published(&state), &labels);
    poller.abort();
    let _ = poller.await;

    let (status, body) = get(&router, "/api/v1/activity").await;
    case(
        "a catch-up with nothing held",
        plain.take(),
        activity(status, &body),
        &labels,
    );
    let (status, body) = get(&router, "/api/v1/activity").await;
    case(
        "a catch-up at once after it",
        plain.take(),
        activity(status, &body),
        &labels,
    );
    tokio::time::sleep(Duration::from_secs(61)).await;
    let (status, body) = get(&router, "/api/v1/activity").await;
    case(
        "a catch-up after the reuse period, nothing changed",
        plain.take(),
        activity(status, &body),
        &labels,
    );
    let edited = fake.seed_at(
        &channel(WRITE_CHANNEL),
        "grace",
        "first draft",
        now - 20 * MINUTE_MS,
    );
    labels.add(&edited, "write-2");
    tokio::time::sleep(Duration::from_secs(61)).await;
    let (status, body) = get(&router, "/api/v1/activity").await;
    case(
        "a catch-up after a message arrived",
        plain.take(),
        activity(status, &body),
        &labels,
    );
    fake.edit(&edited, "second draft, edited");
    let gone = fake.seed_at(
        &channel(THIRD),
        "grace",
        "wrong channel",
        now - 10 * MINUTE_MS,
    );
    labels.add(&gone, "third-2");
    tokio::time::sleep(Duration::from_secs(61)).await;
    let (status, body) = get(&router, "/api/v1/activity").await;
    case(
        "a catch-up after an edit and a message arrived",
        plain.take(),
        activity(status, &body),
        &labels,
    );
    fake.delete(&gone);
    tokio::time::sleep(Duration::from_secs(61)).await;
    let (status, body) = get(&router, "/api/v1/activity").await;
    case(
        "a catch-up after a delete, and nothing added",
        plain.take(),
        activity(status, &body),
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
    let warming = vibe_talk::recent::warm(&state);
    tokio::time::sleep(Duration::from_millis(100)).await;
    case(
        "a second warm-up at once",
        plain.take(),
        json!({ "warming": warming }),
        &labels,
    );

    let report = vibe_talk::probe::probe_channels(state.chat.as_ref(), &state.all_channels()).await;
    case(
        "the startup probe",
        plain.take(),
        json!({ "failure": report.is_failure(), "report": report.render() }),
        &labels,
    );
    let (status, body) = get(&router, "/api/v1/diagnostics").await;
    let chat_checks: Vec<Value> = body["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .filter(|check| {
            check["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("discord."))
        })
        .map(|check| {
            json!({
                "id": check["id"],
                "subject": check["subject"],
                "status": check["status"],
                "summary": check["summary"],
            })
        })
        .collect();
    case(
        "diagnostics",
        plain.take(),
        json!({ "status": status.as_u16(), "chat_checks": chat_checks }),
        &labels,
    );
    Value::Array(cases)
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/channel_heads_default.json")
}

#[tokio::test(start_paused = true)]
async fn a_provider_without_a_head_read_gets_todays_answers_at_todays_cost() {
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
            "{} no longer answers or reads upstream as it did before head reads existed",
            was["case"]
        );
    }
    // The fixture is not vacuous: every caller read upstream, and the reads saw what changed.
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
    assert_eq!(
        upstream("the poller's first tick seeds every channel").len(),
        3
    );
    let quiet = upstream("a quiet tick");
    assert_eq!(quiet.len(), 3, "a page per channel per tick: {quiet:?}");
    assert!(quiet.iter().all(|line| line.starts_with("fetch_page")));
    assert_eq!(
        case("a tick after a message arrived in one channel")["answer"][READ_CHANNEL],
        json!(["deploy finished"])
    );
    assert_eq!(upstream("a catch-up with nothing held").len(), 3);
    assert!(upstream("a catch-up at once after it").is_empty());
    assert_eq!(
        upstream("a catch-up after the reuse period, nothing changed").len(),
        3
    );
    let after = &case("a catch-up after a delete, and nothing added")["answer"];
    let text = after.to_string();
    assert!(text.contains("second draft, edited"), "{text}");
    assert!(!text.contains("wrong channel"), "{text}");
    assert!(!text.contains("first draft\""), "{text}");
    assert_eq!(
        case("a call start's warm-up after the reuse period")["answer"]["warming"],
        3
    );
    assert_eq!(upstream("the startup probe").len(), 3);
    assert!(upstream("diagnostics").len() >= 4);
}

/// A provider that reads heads itself (or says it does not, with `native: false`), answering each
/// channel it is asked about from its fake's newest message and logging the channels of each call.
/// What it reads to answer is not an upstream call of its own: a [`CountingChat`] around it
/// counts what the server asks.
struct Headed {
    inner: Arc<FakeDiscord>,
    native: bool,
    /// What every head says about changes; see [`ChannelHead::unchanged_since_ms`].
    unchanged_since_ms: Mutex<Option<i64>>,
    /// What every head says about its currency; see [`ChannelHead::as_of`].
    as_of: Mutex<Option<String>>,
    asked: Mutex<Vec<Vec<String>>>,
}

impl Headed {
    fn new(inner: Arc<FakeDiscord>, native: bool) -> Self {
        Self {
            inner,
            native,
            unchanged_since_ms: Mutex::new(None),
            as_of: Mutex::new(None),
            asked: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> Vec<Vec<String>> {
        self.asked.lock().expect("log").clone()
    }
}

#[async_trait]
impl ChatClient for Headed {
    fn read_caps(&self, _channel: &ChannelId) -> ReadCaps {
        ReadCaps {
            heads: if self.native {
                Support::Native
            } else {
                Support::Emulated
            },
            ..ReadCaps::default()
        }
    }

    async fn channel_heads(&self, channels: &[ChannelId]) -> HeadAnswers {
        self.asked
            .lock()
            .expect("log")
            .push(channels.iter().map(|c| c.0.clone()).collect());
        let unchanged_since_ms = *self.unchanged_since_ms.lock().expect("setting");
        let as_of = self.as_of.lock().expect("setting").clone();
        let mut answers = Vec::new();
        for channel in channels {
            let head = self.inner.fetch_recent(channel, 1).await.map(|messages| {
                let newest = messages.last();
                ChannelHead {
                    newest_id: newest.map(|m| m.id.clone()),
                    newest_at_ms: newest.and_then(Message::created_ms),
                    activity_ms: None,
                    unchanged_since_ms,
                    as_of: as_of.clone(),
                }
            });
            answers.push((channel.clone(), head));
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

/// remote-ops-audit W12, the router's proof: three channels on two providers are two calls, each
/// provider asked once for its own channels, every answer back in the order asked, and a channel
/// no provider holds refused in its own place and asked of nobody.
#[tokio::test]
async fn the_router_asks_each_provider_once_for_its_own_channels() {
    let fake_a = Arc::new(FakeDiscord::new());
    let fake_b = Arc::new(FakeDiscord::new());
    let a1 = fake_a.seed(&channel("A1"), "ada", "a one");
    let a2 = fake_a.seed(&channel("A2"), "ada", "a two");
    let b1 = fake_b.seed(&channel("B1"), "ada", "b one");
    let headed_a = Arc::new(Headed::new(fake_a, true));
    let headed_b = Arc::new(Headed::new(fake_b, true));
    let counting_a = Arc::new(CountingChat::new(
        Arc::clone(&headed_a) as Arc<dyn ChatClient>
    ));
    let counting_b = Arc::new(CountingChat::new(
        Arc::clone(&headed_b) as Arc<dyn ChatClient>
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
    router.replace_routes(&[routed("A1", "a"), routed("A2", "a"), routed("B1", "b")]);

    let asked = [
        channel("A1"),
        channel("B1"),
        channel("A2"),
        channel("NOWHERE"),
    ];
    let answers = router.channel_heads(&asked).await;

    assert_eq!(counting_a.count("channel_heads"), 1);
    assert_eq!(counting_b.count("channel_heads"), 1);
    assert_eq!(
        counting_a.calls().len() + counting_b.calls().len(),
        2,
        "two calls in all, and no read of any channel"
    );
    assert_eq!(headed_a.asked(), [["A1", "A2"]]);
    assert_eq!(headed_b.asked(), [["B1"]]);
    assert_eq!(
        answers.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
        asked
    );
    let newest = |at: usize| answers[at].1.as_ref().expect("a head").newest_id.clone();
    assert_eq!(newest(0), Some(a1));
    assert_eq!(newest(1), Some(b1));
    assert_eq!(newest(2), Some(a2));
    assert!(
        matches!(answers[3].1, Err(ChatError::Refused(_))),
        "{:?}",
        answers[3].1
    );
}

/// The other side of the same proof: through the router, a provider whose heads are emulated is
/// asked exactly the one-message reads the default is made of, counted as themselves.
#[tokio::test]
async fn through_the_router_an_emulated_head_is_a_one_message_read_per_channel() {
    let fake = Arc::new(FakeDiscord::new());
    fake.seed(&channel("A1"), "ada", "a one");
    fake.seed(&channel("A2"), "ada", "a two");
    let headed = Arc::new(Headed::new(fake, false));
    let counting = Arc::new(CountingChat::new(Arc::clone(&headed) as Arc<dyn ChatClient>));
    let router = ChatRouter::single(Arc::clone(&counting) as Arc<dyn ChatClient>, "a", 0);
    let answers = router.channel_heads(&[channel("A1"), channel("A2")]).await;
    assert!(answers.iter().all(|(_, head)| head.is_ok()));
    assert!(
        headed.asked().is_empty(),
        "an emulated provider's own head read is never asked"
    );
    assert_eq!(counting.count("channel_heads"), 0);
    assert_eq!(counting.count("fetch_recent"), 2);
}

/// remote-ops-audit W12, the refresh permits: each provider reads at most four channels at once
/// for a catch-up or a warm-up, and one provider's queue never holds up another's reads. Five
/// channels on one provider and one on another, every read two seconds: the second provider's
/// read lands at two seconds, not behind the first provider's fifth.
#[tokio::test(start_paused = true)]
async fn each_provider_refreshes_four_channels_at_a_time_without_waiting_for_another() {
    const SLACK_CHANNEL: &str = "C0SLACK001";
    let extra: String = ["3333333333", "4444444444", "5555555555"]
        .iter()
        .map(|id| format!("[[channels]]\nid = \"{id}\"\nlabel = \"{id}\"\nwritable = false\n\n"))
        .collect();
    let toml = format!(
        "[[providers]]\nkey = \"slack\"\nkind = \"slack\"\ntoken = \"xoxb-test\"\n\n\
         [[channels]]\nid = \"{SLACK_CHANNEL}\"\nlabel = \"slack team\"\nwritable = false\n\
         provider = \"slack\"\n\n{}",
        vibe_talk::testing::config_toml().replace("[elevenlabs]", &format!("{extra}[elevenlabs]"))
    );
    let (mut state, discord, _elevenlabs) = vibe_talk::testing::state_from_toml(&toml);
    let slack = Arc::new(FakeDiscord::new());
    slack.seed(&channel(SLACK_CHANNEL), "ada", "from slack");
    let discord_channels = [
        READ_CHANNEL,
        WRITE_CHANNEL,
        "3333333333",
        "4444444444",
        "5555555555",
    ];
    for id in discord_channels {
        discord.seed(&channel(id), "ada", "from discord");
    }
    let read = Duration::from_secs(2);
    let entries = vec![
        ProviderEntry {
            key: "discord".to_owned(),
            namespace: state.config.providers[0].namespace(),
            client: Arc::new(
                CountingChat::new(Arc::clone(&discord) as Arc<dyn ChatClient>).with_delay(read),
            ),
            live_poll_seconds: 0,
        },
        ProviderEntry {
            key: "slack".to_owned(),
            namespace: state.config.providers[1].namespace(),
            client: Arc::new(
                CountingChat::new(Arc::clone(&slack) as Arc<dyn ChatClient>).with_delay(read),
            ),
            live_poll_seconds: 0,
        },
    ];
    let router = ChatRouter::new(entries, state.config.default_provider_key());
    state.replace_providers(Arc::new(router));
    assert_eq!(
        state.providers.key_for(&channel(SLACK_CHANNEL)).as_deref(),
        Some("slack")
    );

    assert_eq!(vibe_talk::recent::warm(&state), 6);
    tokio::time::sleep(read + Duration::from_millis(100)).await;
    let recent = state.live.recent();
    assert!(
        recent.holds(&channel(SLACK_CHANNEL)),
        "the second provider's read did not wait for the first provider's permits"
    );
    let held = discord_channels
        .iter()
        .filter(|id| recent.holds(&channel(id)))
        .count();
    assert_eq!(held, 4, "the first provider still reads four at a time");
    tokio::time::sleep(read).await;
    assert!(discord_channels.iter().all(|id| recent.holds(&channel(id))));
}

/// A server with ten channels on one provider, reading through `counting` around `headed`.
fn ten_channel_server(
    native: bool,
) -> (AppState, Arc<FakeDiscord>, Arc<Headed>, Arc<CountingChat>) {
    let extra: String = (0..8)
        .map(|n| {
            format!(
                "[[channels]]\nid = \"333333333{n}\"\nlabel = \"extra {n}\"\nwritable = false\n\n"
            )
        })
        .collect();
    let toml =
        vibe_talk::testing::config_toml().replace("[elevenlabs]", &format!("{extra}[elevenlabs]"));
    let (mut state, fake, _elevenlabs) = vibe_talk::testing::state_from_toml(&toml);
    for info in state.all_channels() {
        fake.seed(&info.id, "ada", "already there");
    }
    let headed = Arc::new(Headed::new(Arc::clone(&fake), native));
    let counting = Arc::new(CountingChat::new(Arc::clone(&headed) as Arc<dyn ChatClient>));
    state.replace_chat(Arc::clone(&counting) as Arc<dyn ChatClient>);
    (state, fake, headed, counting)
}

/// The poller as `main` starts it, on the provider's own client (profiled, as in production).
fn start_poller(state: &AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(vibe_talk::live::poll_forever(
        state.clone(),
        None,
        Arc::clone(&state.providers.entries()[0].client),
        state.config.chat.default_fetch_limit,
        POLL,
    ))
}

/// What a tick asked of the provider, by method, and forget it.
fn tick_calls(counting: &CountingChat, seen: &mut usize) -> Vec<&'static str> {
    let calls = counting.calls();
    let tick = calls[*seen..].iter().map(|call| call.method).collect();
    *seen = calls.len();
    tick
}

/// remote-ops-audit W12, the poller's proof: with heads read natively, ten quiet channels cost one
/// head read and no page a tick, where they cost ten pages; a channel whose head moved is read,
/// and what arrived in it is published.
#[tokio::test(start_paused = true)]
async fn a_poller_with_native_heads_reads_one_head_list_and_no_page_for_quiet_channels() {
    let (state, fake, headed, counting) = ten_channel_server(true);
    let poller = start_poller(&state);
    let mut seen = 0;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        tick_calls(&counting, &mut seen),
        ["fetch_recent"; 10],
        "the first tick seeds each channel, as before: there is no cursor to compare a head with"
    );
    for _ in 0..2 {
        tokio::time::sleep(POLL).await;
        assert_eq!(
            tick_calls(&counting, &mut seen),
            ["channel_heads"],
            "one head read, and not one page"
        );
    }
    assert_eq!(
        headed.asked().last().map(Vec::len),
        Some(10),
        "every channel in the one call"
    );

    fake.seed(&channel("3333333334"), "grace", "something new");
    tokio::time::sleep(POLL).await;
    assert_eq!(
        tick_calls(&counting, &mut seen),
        ["channel_heads", "fetch_page"]
    );
    let page = counting.calls().last().cloned().expect("a call");
    assert_eq!(
        page.channel,
        Some(channel("3333333334")),
        "only the channel that moved"
    );
    let published: Vec<String> = state
        .live
        .subscribe(&channel("3333333334"), None)
        .replay
        .into_iter()
        .map(|live| live.message.content)
        .collect();
    assert_eq!(published, ["something new"]);
    poller.abort();
}

/// The other half of the poller's proof: a provider whose heads are emulated is never asked for
/// them. Its channels are paged every tick, exactly as before heads existed.
#[tokio::test(start_paused = true)]
async fn a_poller_never_asks_for_heads_a_provider_only_emulates() {
    let (state, _fake, headed, counting) = ten_channel_server(false);
    let poller = start_poller(&state);
    let mut seen = 0;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(tick_calls(&counting, &mut seen), ["fetch_recent"; 10]);
    for _ in 0..2 {
        tokio::time::sleep(POLL).await;
        assert_eq!(tick_calls(&counting, &mut seen), ["fetch_page"; 10]);
    }
    assert!(
        headed.asked().is_empty(),
        "the provider's head read was never called"
    );
    poller.abort();
}

/// A head that cannot prove nothing was added is read as before: one from a copy older than now,
/// and every channel of a provider whose head read fails.
#[tokio::test(start_paused = true)]
async fn a_poller_reads_every_channel_a_head_cannot_vouch_for() {
    let (state, _fake, headed, counting) = ten_channel_server(true);
    let poller = start_poller(&state);
    let mut seen = 0;
    tokio::time::sleep(Duration::from_millis(100)).await;
    tick_calls(&counting, &mut seen);

    *headed.as_of.lock().expect("setting") = Some("2026-10-13T12:00:00Z".to_owned());
    tokio::time::sleep(POLL).await;
    let mut expected = vec!["channel_heads"];
    expected.extend(["fetch_page"; 10]);
    assert_eq!(
        tick_calls(&counting, &mut seen),
        expected,
        "a stale head proves nothing"
    );

    *headed.as_of.lock().expect("setting") = None;
    counting.fail_next(
        "channel_heads",
        None,
        ChatError::Transport("the head read is down".to_owned()),
    );
    tokio::time::sleep(POLL).await;
    assert_eq!(
        tick_calls(&counting, &mut seen),
        expected,
        "a failed head read proves nothing"
    );

    tokio::time::sleep(POLL).await;
    assert_eq!(
        tick_calls(&counting, &mut seen),
        ["channel_heads"],
        "and heads again"
    );
    poller.abort();
}

/// A channel whose page failed keeps being read, whatever its head says, so its backoff and its
/// recovery are what they always were.
#[tokio::test(start_paused = true)]
async fn a_failing_channel_is_read_rather_than_skipped_on_its_head() {
    let (state, fake, headed, counting) = ten_channel_server(true);
    let poller = start_poller(&state);
    let mut seen = 0;
    tokio::time::sleep(Duration::from_millis(100)).await;
    tick_calls(&counting, &mut seen);

    fake.seed(
        &channel(READ_CHANNEL),
        "grace",
        "will fail to arrive at first",
    );
    counting.fail_next(
        "fetch_page",
        None,
        ChatError::Transport("the page read is down".to_owned()),
    );
    tokio::time::sleep(POLL).await;
    assert_eq!(
        tick_calls(&counting, &mut seen),
        ["channel_heads", "fetch_page"]
    );

    // Backed off for two intervals, then read again even though nothing newer has arrived since
    // the failure, and with every other channel still answered by one head read.
    tokio::time::sleep(POLL * 2).await;
    assert_eq!(
        tick_calls(&counting, &mut seen),
        ["channel_heads", "fetch_page"]
    );
    assert_eq!(
        headed.asked().last().map(Vec::len),
        Some(9),
        "the failing channel was not asked about"
    );
    let published: Vec<String> = state
        .live
        .subscribe(&channel(READ_CHANNEL), None)
        .replay
        .into_iter()
        .map(|live| live.message.content)
        .collect();
    assert_eq!(published, ["will fail to arrive at first"]);
    poller.abort();
}
