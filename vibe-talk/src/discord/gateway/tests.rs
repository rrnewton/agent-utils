//! A fake Discord, Gateway and REST both, and the reaction session held against it. `#578
//! discord-live-reactions`.
//!
//! The fake speaks the parts of the Gateway this client uses as Discord documents them: Hello,
//! Identify answered by READY, Resume answered by the missed dispatches and RESUMED or by an
//! Invalid Session, heartbeats acknowledged, Reconnect, and close codes. Its REST side serves
//! `GET /gateway/bot`, channel objects, single messages and message lists, and can answer a read
//! with a 429. Message ids are this file's own, so the owner's process-wide reaction memory never
//! meets another test's.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message as Frame;

use super::*;
use crate::live::{LiveKind, LiveMessage};

const CHANNEL: &str = "100";
const THREAD: &str = "300";
const OWNER: &str = "42";
const SOMEBODY: &str = "43";

/// What the test tells the connection the fake Gateway is holding.
enum Control {
    Send(String),
    /// Drop the TCP connection without a close frame.
    Drop,
    Close(u16),
}

#[derive(Default)]
struct FakeState {
    messages: BTreeMap<(String, String), Value>,
    channels: BTreeMap<String, Value>,
    /// How many of the next single-message reads answer 429.
    rate_limit_next: u32,
    rate_limited: u32,
    requests: Vec<String>,
    heartbeat_ms: u64,
    mute_acks: bool,
    seq: u64,
    /// Every reaction dispatch, with its sequence number, for a resume to replay.
    log: Vec<(u64, String)>,
    sessions: u32,
    session: Option<String>,
    resumable: bool,
    identifies: u32,
    resumes: Vec<Value>,
    close_on_identify: Option<u16>,
    connection: Option<mpsc::UnboundedSender<Control>>,
    connections: u32,
    heartbeats: u32,
}

struct Fake {
    state: Arc<Mutex<FakeState>>,
    rest: SocketAddr,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn locked(state: &Mutex<FakeState>) -> std::sync::MutexGuard<'_, FakeState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn text(value: &Value) -> Frame {
    Frame::text(value.to_string())
}

impl Fake {
    async fn start() -> Self {
        let state = Arc::new(Mutex::new(FakeState {
            heartbeat_ms: 45_000,
            resumable: true,
            ..FakeState::default()
        }));
        let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("gateway listener");
        let gateway = gateway_listener.local_addr().expect("gateway address");
        let rest_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("rest listener");
        let rest = rest_listener.local_addr().expect("rest address");
        let shared = Arc::clone(&state);
        let app = Router::new().fallback(move |method: Method, uri: Uri| {
            let state = Arc::clone(&shared);
            async move { answer(&state, &method, &uri, gateway) }
        });
        let serving_rest = tokio::spawn(async move {
            axum::serve(rest_listener, app).await.expect("rest server");
        });
        let shared = Arc::clone(&state);
        let serving_gateway = tokio::spawn(async move {
            while let Ok((stream, _)) = gateway_listener.accept().await {
                tokio::spawn(hold(stream, Arc::clone(&shared), gateway));
            }
        });
        Self {
            state,
            rest,
            tasks: vec![serving_rest, serving_gateway],
        }
    }

    fn put_message(&self, channel: &str, id: &str, reactions: Option<Value>) {
        let mut message = json!({
            "id": id,
            "channel_id": channel,
            "author": {"id": "9", "username": "ada"},
            "content": "hello",
            "timestamp": "2026-10-27T06:00:00.000000+00:00",
        });
        if let Some(reactions) = reactions {
            message["reactions"] = reactions;
        }
        locked(&self.state)
            .messages
            .insert((channel.to_owned(), id.to_owned()), message);
    }

    fn put_thread(&self, id: &str, parent: &str) {
        locked(&self.state).channels.insert(
            id.to_owned(),
            json!({"id": id, "type": 11, "parent_id": parent, "name": "a thread",
                   "last_message_id": null, "message_count": 1}),
        );
    }

    /// Dispatch `name` with `data`: to the connection held now, and to the log a resume replays.
    fn dispatch(&self, name: &str, data: Value) {
        let mut state = locked(&self.state);
        state.seq += 1;
        let payload = json!({"op": 0, "t": name, "s": state.seq, "d": data}).to_string();
        let seq = state.seq;
        state.log.push((seq, payload.clone()));
        if let Some(connection) = &state.connection {
            let _ = connection.send(Control::Send(payload));
        }
    }

    fn control(&self, control: Control) {
        if let Some(connection) = locked(&self.state).connection.take() {
            let _ = connection.send(control);
        }
    }

    fn requests(&self, path: &str) -> usize {
        locked(&self.state)
            .requests
            .iter()
            .filter(|request| request.as_str() == path)
            .count()
    }

    fn config(&self) -> crate::config::Config {
        let mut config = crate::testing::config();
        let discord = config.discord_mut().expect("discord provider");
        discord.api_base = format!("http://{}", self.rest);
        discord.owner_user_id = Some(OWNER.to_owned());
        discord.gateway_reactions = true;
        config
    }
}

/// The fake's REST side.
fn answer(
    state: &Mutex<FakeState>,
    method: &Method,
    uri: &Uri,
    gateway: SocketAddr,
) -> axum::response::Response {
    let mut state = locked(state);
    let path = uri.path().to_owned();
    state.requests.push(path.clone());
    if method != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segments.as_slice() {
        ["gateway", "bot"] => Json(json!({
            "url": format!("ws://{gateway}"),
            "shards": 1,
            "session_start_limit": {"total": 1000, "remaining": 999, "reset_after": 1000,
                                    "max_concurrency": 1},
        }))
        .into_response(),
        ["channels", channel] => match state.channels.get(*channel) {
            Some(value) => Json(value.clone()).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({"message": "Unknown Channel", "code": 10003})),
            )
                .into_response(),
        },
        ["channels", channel, "messages", id] => {
            if state.rate_limit_next > 0 {
                state.rate_limit_next -= 1;
                state.rate_limited += 1;
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "1"), ("x-ratelimit-scope", "user")],
                    Json(
                        json!({"message": "You are being rate limited.", "retry_after": 0.05,
                                "global": false}),
                    ),
                )
                    .into_response();
            }
            match state
                .messages
                .get(&((*channel).to_owned(), (*id).to_owned()))
            {
                Some(message) => Json(message.clone()).into_response(),
                None => (
                    StatusCode::NOT_FOUND,
                    Json(json!({"message": "Unknown Message", "code": 10008})),
                )
                    .into_response(),
            }
        }
        ["channels", channel, "messages"] => {
            let limit = uri
                .query()
                .and_then(|query| {
                    query
                        .split('&')
                        .find_map(|pair| pair.strip_prefix("limit="))
                        .and_then(|limit| limit.parse::<usize>().ok())
                })
                .unwrap_or(50);
            let mut list: Vec<Value> = state
                .messages
                .iter()
                .filter(|((held, _), _)| held == channel)
                .map(|(_, message)| message.clone())
                .collect();
            list.sort_by_key(|message| {
                std::cmp::Reverse(
                    message["id"]
                        .as_str()
                        .and_then(|id| id.parse::<u64>().ok())
                        .unwrap_or(0),
                )
            });
            list.truncate(limit);
            Json(Value::Array(list)).into_response()
        }
        _ => (StatusCode::NOT_FOUND, Json(json!({"code": 0}))).into_response(),
    }
}

/// One connection to the fake Gateway, from Hello to its end.
async fn hold(stream: tokio::net::TcpStream, state: Arc<Mutex<FakeState>>, gateway: SocketAddr) {
    let Ok(socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut sink, mut frames) = socket.split();
    let (sender, mut controls) = mpsc::unbounded_channel();
    let heartbeat_ms = {
        let mut held = locked(&state);
        held.connections += 1;
        held.connection = Some(sender.clone());
        held.heartbeat_ms
    };
    if sink
        .send(text(
            &json!({"op": 10, "d": {"heartbeat_interval": heartbeat_ms}}),
        ))
        .await
        .is_err()
    {
        return;
    }
    let opening = loop {
        match frames.next().await {
            Some(Ok(Frame::Text(raw))) => {
                let Ok(payload) = serde_json::from_str::<Value>(raw.as_str()) else {
                    continue;
                };
                match payload["op"].as_u64() {
                    Some(2 | 6) => break payload,
                    Some(1) => {
                        let _ = sink.send(text(&json!({"op": 11}))).await;
                    }
                    _ => {}
                }
            }
            Some(Ok(_)) => {}
            _ => return,
        }
    };
    let mut outgoing: Vec<Frame> = Vec::new();
    let mut end_after = false;
    {
        let mut held = locked(&state);
        if opening["op"] == 2 {
            held.identifies += 1;
            assert_eq!(
                opening["d"]["intents"],
                json!(INTENTS),
                "the reaction intents only"
            );
            if let Some(code) = held.close_on_identify {
                outgoing.push(Frame::Close(Some(CloseFrame {
                    code: CloseCode::from(code),
                    reason: "".into(),
                })));
                end_after = true;
            } else {
                held.sessions += 1;
                let id = format!("session-{}", held.sessions);
                held.session = Some(id.clone());
                held.seq += 1;
                outgoing.push(text(&json!({
                    "op": 0, "t": "READY", "s": held.seq,
                    "d": {"v": 10, "session_id": id, "resume_gateway_url": format!("ws://{gateway}"),
                          "user": {"id": "1000000000000000009", "username": "bot"}, "guilds": []},
                })));
            }
        } else {
            held.resumes.push(opening["d"].clone());
            let matches = held.session.as_deref() == opening["d"]["session_id"].as_str();
            if held.resumable && matches {
                let after = opening["d"]["seq"].as_u64().unwrap_or(0);
                for (seq, payload) in &held.log {
                    if *seq > after {
                        outgoing.push(Frame::text(payload.clone()));
                    }
                }
                held.seq += 1;
                outgoing.push(text(
                    &json!({"op": 0, "t": "RESUMED", "s": held.seq, "d": null}),
                ));
            } else {
                held.session = None;
                outgoing.push(text(&json!({"op": 9, "d": false})));
            }
        }
    }
    for frame in outgoing {
        if sink.send(frame).await.is_err() {
            return;
        }
    }
    if end_after {
        return;
    }
    loop {
        tokio::select! {
            frame = frames.next() => match frame {
                Some(Ok(Frame::Text(raw))) => {
                    let Ok(payload) = serde_json::from_str::<Value>(raw.as_str()) else {
                        continue;
                    };
                    if payload["op"] == 1 {
                        let mute = {
                            let mut held = locked(&state);
                            held.heartbeats += 1;
                            held.mute_acks
                        };
                        if !mute && sink.send(text(&json!({"op": 11}))).await.is_err() {
                            break;
                        }
                    }
                }
                Some(Ok(Frame::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            control = controls.recv() => match control {
                Some(Control::Send(payload)) => {
                    if sink.send(Frame::text(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Control::Close(code)) => {
                    let _ = sink.send(Frame::Close(Some(CloseFrame {
                        code: CloseCode::from(code),
                        reason: "".into(),
                    }))).await;
                    break;
                }
                Some(Control::Drop) | None => break,
            },
        }
    }
    let mut held = locked(&state);
    if held
        .connection
        .as_ref()
        .is_some_and(|current| current.same_channel(&sender))
    {
        held.connection = None;
    }
}

fn timing() -> Timing {
    Timing {
        coalesce: Duration::from_millis(30),
        resume_wait: Duration::from_millis(20),
        identify_wait: Duration::from_millis(20),
        max_wait: Duration::from_millis(200),
        stable: Duration::from_millis(50),
        connect_timeout: Duration::from_secs(2),
    }
}

/// A running session against `fake`, and the hub it publishes into, watched on [`CHANNEL`].
struct Running {
    hub: Arc<LiveHub>,
    updates: broadcast::Receiver<LiveMessage>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn start(fake: &Fake) -> Running {
    let hub = Arc::new(LiveHub::new());
    let updates = hub.subscribe(&ChannelId(CHANNEL.into()), None).receiver;
    let config = fake.config();
    let chat: Arc<dyn ChatClient> =
        Arc::new(HttpDiscordClient::new(config.discord().expect("discord")).expect("client"));
    let channels: Channels = Arc::new(|| vec![ChannelId(CHANNEL.into())]);
    let running_hub = Arc::clone(&hub);
    let task = tokio::spawn(async move {
        run(
            config.discord().expect("discord"),
            running_hub,
            chat,
            channels,
            timing(),
        )
        .await;
    });
    Running { hub, updates, task }
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn ready(fake: &Fake, running: &Running, identifies: u32) {
    eventually("a ready session", || {
        locked(&fake.state).identifies >= identifies
            && locked(&fake.state).connection.is_some()
            && running.hub.reactions_pushed(&ChannelId(CHANNEL.into()))
    })
    .await;
}

async fn next_update(updates: &mut broadcast::Receiver<LiveMessage>) -> LiveMessage {
    let live = tokio::time::timeout(Duration::from_secs(3), updates.recv())
        .await
        .expect("an update within three seconds")
        .expect("the hub is open");
    assert_eq!(live.kind, LiveKind::Update);
    live
}

fn unicode(name: &str, count: u64) -> Value {
    json!({"count": count, "me": false, "emoji": {"id": null, "name": name}})
}

fn added(channel: &str, message: &str, user: &str, emoji: Value) -> Value {
    json!({"user_id": user, "channel_id": channel, "message_id": message, "guild_id": "1",
           "emoji": emoji, "burst": false, "type": 0})
}

fn shown(live: &LiveMessage) -> Vec<(String, bool, Option<String>, u32, bool)> {
    live.message
        .reactions
        .as_ref()
        .expect("an update says where reactions stand")
        .iter()
        .map(|reaction| {
            (
                reaction.emoji.clone(),
                reaction.custom,
                reaction.custom_id.clone(),
                reaction.count,
                reaction.owner_reacted,
            )
        })
        .collect()
}

#[test]
fn the_four_reaction_events_are_read_and_anything_else_is_not() {
    let add = parse_reaction_event(
        "MESSAGE_REACTION_ADD",
        &added("1", "2", "3", json!({"id": null, "name": "👍"})),
    );
    assert_eq!(
        add,
        Some(ReactionEvent {
            channel: "1".into(),
            message: "2".into(),
            change: ReactionChange::Added {
                user: "3".into(),
                emoji: "👍".into()
            },
        })
    );
    let custom = parse_reaction_event(
        "MESSAGE_REACTION_REMOVE",
        &json!({"user_id": "3", "channel_id": "1", "message_id": "2",
                "emoji": {"id": "777", "name": "party"}}),
    );
    assert_eq!(
        custom.map(|event| event.change),
        Some(ReactionChange::Removed {
            user: "3".into(),
            emoji: "777".into()
        })
    );
    assert_eq!(
        parse_reaction_event(
            "MESSAGE_REACTION_REMOVE_ALL",
            &json!({"channel_id": "1", "message_id": "2"})
        )
        .map(|event| event.change),
        Some(ReactionChange::RemovedAll)
    );
    assert_eq!(
        parse_reaction_event(
            "MESSAGE_REACTION_REMOVE_EMOJI",
            &json!({"channel_id": "1", "message_id": "2", "emoji": {"id": null, "name": "👀"}})
        )
        .map(|event| event.change),
        Some(ReactionChange::RemovedEmoji {
            emoji: "👀".into()
        })
    );
    // Not a reaction event, or one missing what it needs: nothing.
    assert_eq!(
        parse_reaction_event("MESSAGE_CREATE", &json!({"channel_id": "1", "id": "2"})),
        None
    );
    assert_eq!(
        parse_reaction_event(
            "MESSAGE_REACTION_ADD",
            &json!({"channel_id": "1", "message_id": "2", "emoji": {"name": "👍"}})
        ),
        None,
        "an add without its user"
    );
    assert_eq!(
        parse_reaction_event(
            "MESSAGE_REACTION_ADD",
            &json!({"user_id": "3", "channel_id": "1", "message_id": "2",
                    "emoji": {"id": null, "name": null}})
        ),
        None,
        "an emoji with neither id nor name"
    );
}

#[test]
fn the_owners_own_reactions_mark_every_read_of_the_message_until_taken_back() {
    let message = "1578000000000000001";
    let event = |change| ReactionEvent {
        channel: CHANNEL.into(),
        message: message.into(),
        change,
    };
    let read = || {
        let mut reactions = crate::discord::http::parse_reactions(Some(&json!([
            unicode("👍", 2),
            {"count": 1, "emoji": {"id": "777", "name": "party"}},
        ])))
        .expect("an array is an answer");
        mark_owner_reactions(message, &mut reactions);
        reactions
            .iter()
            .map(|reaction| reaction.owner_reacted)
            .collect::<Vec<_>>()
    };
    assert_eq!(read(), [false, false]);
    // Somebody else's reaction is not the owner's; neither is any without an owner configured.
    note_owner_reaction(
        &event(ReactionChange::Added {
            user: SOMEBODY.into(),
            emoji: "👍".into(),
        }),
        Some(OWNER),
    );
    note_owner_reaction(
        &event(ReactionChange::Added {
            user: OWNER.into(),
            emoji: "👍".into(),
        }),
        None,
    );
    assert_eq!(read(), [false, false]);
    note_owner_reaction(
        &event(ReactionChange::Added {
            user: OWNER.into(),
            emoji: "👍".into(),
        }),
        Some(OWNER),
    );
    note_owner_reaction(
        &event(ReactionChange::Added {
            user: OWNER.into(),
            emoji: "777".into(),
        }),
        Some(OWNER),
    );
    assert_eq!(read(), [true, true], "a custom emoji is known by its id");
    note_owner_reaction(
        &event(ReactionChange::Removed {
            user: OWNER.into(),
            emoji: "👍".into(),
        }),
        Some(OWNER),
    );
    assert_eq!(read(), [false, true]);
    note_owner_reaction(
        &event(ReactionChange::RemovedEmoji {
            emoji: "777".into(),
        }),
        Some(OWNER),
    );
    assert_eq!(read(), [false, false]);
    note_owner_reaction(
        &event(ReactionChange::Added {
            user: OWNER.into(),
            emoji: "👍".into(),
        }),
        Some(OWNER),
    );
    note_owner_reaction(&event(ReactionChange::RemovedAll), Some(OWNER));
    assert_eq!(read(), [false, false]);
}

#[test]
fn addresses_close_codes_and_waits_follow_discords_rules() {
    assert_eq!(
        socket_url("wss://gateway.discord.gg", true).as_deref(),
        Some("wss://gateway.discord.gg/?v=10&encoding=json")
    );
    assert_eq!(
        socket_url("wss://gateway-us-east1-b.discord.gg/?v=9", true).as_deref(),
        Some("wss://gateway-us-east1-b.discord.gg/?v=10&encoding=json"),
        "the version and encoding are this client's own"
    );
    assert_eq!(socket_url("ws://gateway.discord.gg", true), None);
    assert_eq!(socket_url("https://gateway.discord.gg", true), None);
    assert!(socket_url("ws://127.0.0.1:1", false).is_some());

    assert!(matches!(after_close(4004), Ending::Fatal(_)));
    for code in 4010..=4014 {
        assert!(matches!(after_close(code), Ending::Fatal(_)), "{code}");
    }
    assert_eq!(after_close(4007), Ending::Identify);
    assert_eq!(after_close(4009), Ending::Identify);
    for code in [1000, 1001, 1006, 4000, 4001, 4002, 4003, 4005, 4008] {
        assert_eq!(after_close(code), Ending::Resume, "{code}");
    }

    let floor = Duration::from_secs(5);
    let max = Duration::from_secs(600);
    assert_eq!(wait(0, floor, max), floor);
    assert_eq!(wait(1, floor, max), floor);
    assert_eq!(wait(2, floor, max), floor * 2);
    assert_eq!(wait(4, floor, max), floor * 8);
    assert_eq!(wait(40, floor, max), max);
}

#[tokio::test]
async fn an_added_reaction_reaches_the_page_well_within_a_second() {
    let fake = Fake::start().await;
    let message = "1578000000000000101";
    fake.put_message(CHANNEL, message, None);
    let mut running = start(&fake);
    ready(&fake, &running, 1).await;

    fake.put_message(CHANNEL, message, Some(json!([unicode("👍", 1)])));
    let sent = std::time::Instant::now();
    fake.dispatch(
        "MESSAGE_REACTION_ADD",
        added(CHANNEL, message, OWNER, json!({"id": null, "name": "👍"})),
    );
    let live = next_update(&mut running.updates).await;
    let latency = sent.elapsed();
    assert_eq!(live.message.id.as_str(), message);
    assert_eq!(shown(&live), [("👍".into(), false, None, 1, true)]);
    // The coalescing wait (30 ms here, 150 ms in the server) plus one read of the message.
    assert!(
        latency < Duration::from_secs(1),
        "event to update took {latency:?}"
    );
    eprintln!("discord-live-reactions: event to published update {latency:?} (coalesce 30 ms)");
    assert_eq!(
        fake.requests(&format!("/channels/{CHANNEL}/messages/{message}")),
        1
    );
}

#[tokio::test]
async fn removals_of_one_all_and_one_emoji_each_publish_the_set_that_is_left() {
    let fake = Fake::start().await;
    let message = "1578000000000000201";
    fake.put_message(CHANNEL, message, None);
    let mut running = start(&fake);
    ready(&fake, &running, 1).await;
    let thumbs = || json!({"id": null, "name": "👍"});

    fake.put_message(
        CHANNEL,
        message,
        Some(json!([unicode("👍", 2), unicode("👀", 1)])),
    );
    fake.dispatch(
        "MESSAGE_REACTION_ADD",
        added(CHANNEL, message, OWNER, thumbs()),
    );
    assert_eq!(
        shown(&next_update(&mut running.updates).await),
        [
            ("👍".into(), false, None, 2, true),
            ("👀".into(), false, None, 1, false)
        ]
    );

    fake.put_message(
        CHANNEL,
        message,
        Some(json!([unicode("👍", 1), unicode("👀", 1)])),
    );
    fake.dispatch(
        "MESSAGE_REACTION_REMOVE",
        added(CHANNEL, message, OWNER, thumbs()),
    );
    assert_eq!(
        shown(&next_update(&mut running.updates).await),
        [
            ("👍".into(), false, None, 1, false),
            ("👀".into(), false, None, 1, false)
        ],
        "the owner's mark goes with his reaction; somebody else's stays"
    );

    fake.put_message(CHANNEL, message, Some(json!([unicode("👀", 1)])));
    fake.dispatch(
        "MESSAGE_REACTION_REMOVE_EMOJI",
        json!({"channel_id": CHANNEL, "message_id": message, "emoji": thumbs()}),
    );
    assert_eq!(
        shown(&next_update(&mut running.updates).await),
        [("👀".into(), false, None, 1, false)]
    );

    // Discord leaves the field out when the last one goes: published as "none", outright.
    fake.put_message(CHANNEL, message, None);
    fake.dispatch(
        "MESSAGE_REACTION_REMOVE_ALL",
        json!({"channel_id": CHANNEL, "message_id": message}),
    );
    let live = next_update(&mut running.updates).await;
    assert_eq!(live.message.reactions, Some(Vec::new()));
}

#[tokio::test]
async fn a_custom_emoji_keeps_its_name_and_its_id() {
    let fake = Fake::start().await;
    let message = "1578000000000000301";
    fake.put_message(CHANNEL, message, None);
    let mut running = start(&fake);
    ready(&fake, &running, 1).await;
    let party = json!({"id": "1578999", "name": "party_parrot", "animated": true});

    fake.put_message(
        CHANNEL,
        message,
        Some(json!([{"count": 1, "me": false, "emoji": party}])),
    );
    fake.dispatch(
        "MESSAGE_REACTION_ADD",
        added(CHANNEL, message, SOMEBODY, party.clone()),
    );
    assert_eq!(
        shown(&next_update(&mut running.updates).await),
        [(
            "party_parrot".into(),
            true,
            Some("1578999".into()),
            1,
            false
        )]
    );

    fake.put_message(
        CHANNEL,
        message,
        Some(json!([{"count": 2, "me": false, "emoji": party}])),
    );
    fake.dispatch(
        "MESSAGE_REACTION_ADD",
        added(CHANNEL, message, OWNER, party),
    );
    assert_eq!(
        shown(&next_update(&mut running.updates).await),
        [("party_parrot".into(), true, Some("1578999".into()), 2, true)]
    );
}

#[tokio::test]
async fn a_burst_of_events_on_one_message_costs_one_read_and_one_update() {
    let fake = Fake::start().await;
    let message = "1578000000000000401";
    fake.put_message(CHANNEL, message, None);
    let mut running = start(&fake);
    ready(&fake, &running, 1).await;
    fake.put_message(CHANNEL, message, Some(json!([unicode("🎉", 5)])));
    for user in ["51", "52", "53", "54", "55"] {
        fake.dispatch(
            "MESSAGE_REACTION_ADD",
            added(CHANNEL, message, user, json!({"id": null, "name": "🎉"})),
        );
    }
    assert_eq!(
        shown(&next_update(&mut running.updates).await),
        [("🎉".into(), false, None, 5, false)]
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        fake.requests(&format!("/channels/{CHANNEL}/messages/{message}")),
        1
    );
    assert!(
        running.updates.try_recv().is_err(),
        "one update for the burst"
    );
}

#[tokio::test]
async fn a_reaction_in_a_thread_is_published_on_its_parent_channel() {
    let fake = Fake::start().await;
    let message = "1578000000000000501";
    fake.put_thread(THREAD, CHANNEL);
    fake.put_message(THREAD, message, None);
    let mut running = start(&fake);
    ready(&fake, &running, 1).await;

    fake.put_message(THREAD, message, Some(json!([unicode("✅", 1)])));
    fake.dispatch(
        "MESSAGE_REACTION_ADD",
        added(THREAD, message, OWNER, json!({"id": null, "name": "✅"})),
    );
    let live = next_update(&mut running.updates).await;
    assert_eq!(live.message.id.as_str(), message);
    assert_eq!(shown(&live), [("✅".into(), false, None, 1, true)]);

    // An event in a channel that is neither configured nor a thread of one reads nothing more
    // than that channel, once.
    fake.dispatch(
        "MESSAGE_REACTION_ADD",
        added(
            "999",
            "1578000000000000502",
            OWNER,
            json!({"id": null, "name": "✅"}),
        ),
    );
    fake.dispatch(
        "MESSAGE_REACTION_ADD",
        added(
            "999",
            "1578000000000000503",
            OWNER,
            json!({"id": null, "name": "✅"}),
        ),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fake.requests("/channels/999"), 1);
    assert_eq!(
        fake.requests("/channels/999/messages/1578000000000000502"),
        0
    );
    assert!(running.updates.try_recv().is_err());
}

#[tokio::test]
async fn a_dropped_connection_resumes_and_what_was_missed_is_replayed() {
    let fake = Fake::start().await;
    let message = "1578000000000000601";
    fake.put_message(CHANNEL, message, None);
    let mut running = start(&fake);
    ready(&fake, &running, 1).await;

    fake.control(Control::Drop);
    // While nothing is connected: only the log a resume replays has it.
    fake.put_message(CHANNEL, message, Some(json!([unicode("👍", 1)])));
    fake.dispatch(
        "MESSAGE_REACTION_ADD",
        added(
            CHANNEL,
            message,
            SOMEBODY,
            json!({"id": null, "name": "👍"}),
        ),
    );
    let live = next_update(&mut running.updates).await;
    assert_eq!(shown(&live), [("👍".into(), false, None, 1, false)]);
    let state = locked(&fake.state);
    assert_eq!(state.identifies, 1, "a resume, not a second identify");
    assert_eq!(state.resumes.len(), 1);
    assert_eq!(state.resumes[0]["session_id"], "session-1");
    assert_eq!(
        state.resumes[0]["seq"], 1,
        "the READY was the last dispatch seen"
    );
}

#[tokio::test]
async fn discords_reconnect_request_is_resumed_too() {
    let fake = Fake::start().await;
    let mut running = start(&fake);
    ready(&fake, &running, 1).await;
    let connection = locked(&fake.state).connection.clone().expect("connected");
    let _ = connection.send(Control::Send(json!({"op": 7, "d": null}).to_string()));
    eventually("a resume after op 7", || {
        locked(&fake.state).resumes.len() == 1
    })
    .await;
    let message = "1578000000000000651";
    fake.put_message(CHANNEL, message, Some(json!([unicode("👍", 1)])));
    ready(&fake, &running, 1).await;
    fake.dispatch(
        "MESSAGE_REACTION_ADD",
        added(
            CHANNEL,
            message,
            SOMEBODY,
            json!({"id": null, "name": "👍"}),
        ),
    );
    assert_eq!(
        next_update(&mut running.updates).await.message.id.as_str(),
        message
    );
    assert_eq!(locked(&fake.state).identifies, 1);
}

#[tokio::test]
async fn a_session_that_cannot_resume_identifies_again_and_reconciles_the_gap() {
    let fake = Fake::start().await;
    let message = "1578000000000000701";
    fake.put_message(CHANNEL, message, Some(json!([unicode("👍", 1)])));
    let mut running = start(&fake);
    ready(&fake, &running, 1).await;
    // The first session's reconcile learned where reactions stand, silently.
    eventually("the first reconcile", || {
        fake.requests(&format!("/channels/{CHANNEL}/messages")) >= 1
    })
    .await;
    assert!(running.updates.try_recv().is_err());

    locked(&fake.state).resumable = false;
    fake.control(Control::Drop);
    // Changed while no session was up, and never dispatched: only a reconcile can find it.
    fake.put_message(
        CHANNEL,
        message,
        Some(json!([unicode("👍", 1), unicode("🚀", 3)])),
    );
    let live = next_update(&mut running.updates).await;
    assert_eq!(live.message.id.as_str(), message);
    assert_eq!(
        shown(&live),
        [
            ("👍".into(), false, None, 1, false),
            ("🚀".into(), false, None, 3, false)
        ]
    );
    let state = locked(&fake.state);
    assert_eq!(state.resumes.len(), 1, "it tried to resume first");
    assert_eq!(state.identifies, 2);
}

#[tokio::test]
async fn a_rate_limited_read_waits_as_discord_says_and_still_publishes() {
    let fake = Fake::start().await;
    let message = "1578000000000000801";
    fake.put_message(CHANNEL, message, None);
    let mut running = start(&fake);
    ready(&fake, &running, 1).await;
    {
        let mut state = locked(&fake.state);
        state.rate_limit_next = 1;
    }
    fake.put_message(CHANNEL, message, Some(json!([unicode("👍", 1)])));
    fake.dispatch(
        "MESSAGE_REACTION_ADD",
        added(
            CHANNEL,
            message,
            SOMEBODY,
            json!({"id": null, "name": "👍"}),
        ),
    );
    let live = next_update(&mut running.updates).await;
    assert_eq!(shown(&live), [("👍".into(), false, None, 1, false)]);
    assert_eq!(locked(&fake.state).rate_limited, 1);
    assert_eq!(
        fake.requests(&format!("/channels/{CHANNEL}/messages/{message}")),
        2,
        "the 429 and the retry"
    );
}

#[tokio::test]
async fn a_refused_token_stops_the_session_and_hands_reactions_back_to_the_poller() {
    let fake = Fake::start().await;
    let mut running = start(&fake);
    ready(&fake, &running, 1).await;
    locked(&fake.state).close_on_identify = Some(4004);
    // A session that cannot resume, so the next connection identifies and is refused.
    locked(&fake.state).resumable = false;
    fake.control(Control::Close(4009));
    let task = &mut running.task;
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the session stops")
        .expect("without panicking");
    assert!(!running.hub.reactions_pushed(&ChannelId(CHANNEL.into())));
    assert_eq!(locked(&fake.state).identifies, 2);
}

#[tokio::test]
async fn an_unacknowledged_heartbeat_is_a_dead_connection_and_is_resumed() {
    let fake = Fake::start().await;
    {
        let mut state = locked(&fake.state);
        state.heartbeat_ms = 40;
        state.mute_acks = true;
    }
    let running = start(&fake);
    ready(&fake, &running, 1).await;
    eventually("a resume after a silent heartbeat", || {
        !locked(&fake.state).resumes.is_empty()
    })
    .await;
    let state = locked(&fake.state);
    assert!(state.heartbeats >= 1);
    assert_eq!(state.identifies, 1);
}
