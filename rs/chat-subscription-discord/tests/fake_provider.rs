//! Deterministic fake-provider tests: the Discord backend driven through the host-side
//! `ChatSubscription` (which enforces sequence, boundary, channel and sender authority), and the
//! outbound helper, both against an in-memory Discord. No network, no processes.

use std::sync::{Arc, Mutex};

use chat_subscription::{
    BackendConfiguration, ChannelId, ChatSubscription, CommittableEvent, ProviderCursor,
    RejectionReason, SenderId, SubscribeRequest, SubscriptionItem,
};
use chat_subscription_discord::ids::Snowflake;
use chat_subscription_discord::outbound::{handle_request, OutboundConfig};
use chat_subscription_discord::plugin::{DiscordBackend, CONFIG_SCHEMA};
use chat_subscription_discord::transport::{DiscordMessage, DiscordRest, FakeRest, RestError};
use serde_json::{json, Value};

const CHANNEL: u64 = 1_566_504_459_499_217_097;
const OURS: u64 = 1_545_796_614_826_889_226;
const OTHER: u64 = 1_532_416_065_114_607_829;
const OWNER: u64 = 111_222_333_444_555_666;
const STRANGER: u64 = 777_888_999_000_111_222;
const RANDOM_BOT: u64 = 555_000_555_000_555_000;

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<FakeRest>>);

impl Shared {
    fn with<T>(&self, action: impl FnOnce(&mut FakeRest) -> T) -> T {
        action(&mut self.0.lock().expect("fake lock"))
    }
}

impl DiscordRest for Shared {
    fn list_messages(
        &mut self,
        channel: Snowflake,
        after: Option<Snowflake>,
        limit: u8,
    ) -> Result<Vec<DiscordMessage>, RestError> {
        self.with(|fake| fake.list_messages(channel, after, limit))
    }
    fn create_message(
        &mut self,
        channel: Snowflake,
        content: &str,
        reply_to: Option<Snowflake>,
        nonce: &str,
    ) -> Result<Snowflake, RestError> {
        self.with(|fake| fake.create_message(channel, content, reply_to, nonce))
    }
    fn add_reaction(
        &mut self,
        channel: Snowflake,
        message: Snowflake,
        emoji: &str,
    ) -> Result<(), RestError> {
        self.with(|fake| fake.add_reaction(channel, message, emoji))
    }
}

fn message(
    id: u64,
    author: u64,
    bot: bool,
    content: &str,
    reply_to: Option<u64>,
) -> DiscordMessage {
    DiscordMessage {
        id: Snowflake::from_u64(id),
        channel_id: Snowflake::from_u64(CHANNEL),
        author_id: Snowflake::from_u64(author),
        author_is_bot: bot,
        content: content.to_owned(),
        timestamp: "2026-11-15T16:45:00.123000+00:00".to_owned(),
        reply_to: reply_to.map(Snowflake::from_u64),
    }
}

fn request(resume: Option<&str>) -> SubscribeRequest {
    let data = json!({
        "bot_user_id": OURS.to_string(),
        "host": "newton-fedora",
        "aliases": ["newton-team"],
        "local_agents": ["coordinator", "lane-a"],
        "peer_bots": {OTHER.to_string(): "deepscry@lead-host"},
        "token_file": "/nonexistent/.env",
        "poll_seconds": 2,
        "heartbeat_seconds": 5
    });
    SubscribeRequest::new(
        vec![ChannelId::new(CHANNEL.to_string()).expect("channel")],
        vec![SenderId::new(OWNER.to_string()).expect("owner")],
        resume.map(|cursor| ProviderCursor::new(cursor).expect("cursor")),
    )
    .expect("request")
    .with_relay_senders(vec![SenderId::new(OTHER.to_string()).expect("peer")])
    .expect("relay senders")
    .with_relay_channels(vec![ChannelId::new(CHANNEL.to_string()).expect("channel")])
    .expect("relay channels")
    .with_backend_configuration(
        BackendConfiguration::new(CONFIG_SCHEMA, data.as_object().cloned().expect("object"))
            .expect("configuration"),
    )
}

fn make_backend(shared: &Shared) -> DiscordBackend {
    let shared = shared.clone();
    DiscordBackend::new(Box::new(move |_config| {
        Ok(Box::new(shared.clone()) as Box<dyn DiscordRest>)
    }))
}

/// Receive one batch through the host, commit it, and return its single event and cursor.
fn take(subscription: &mut ChatSubscription) -> (CommittableEvent, String) {
    match subscription.next_item().expect("item") {
        Some(SubscriptionItem::Batch(batch)) => {
            subscription.commit_durable(&batch).expect("commit");
            assert_eq!(batch.events().len(), 1);
            (
                batch.events()[0].clone(),
                batch.cursor().as_str().to_owned(),
            )
        }
        other => panic!("expected a batch, got {other:?}"),
    }
}

fn created(event: &CommittableEvent) -> (&str, &str, &str, Value) {
    match event {
        CommittableEvent::MessageCreated(message) => (
            message.sender_id().as_str(),
            message.text(),
            message.thread_id().as_str(),
            Value::Object(message.provider_payload().expect("payload").data().clone()),
        ),
        other => panic!("expected message_created, got {other:?}"),
    }
}

#[test]
fn addressing_cursoring_and_relay_through_the_host() {
    let shared = Shared::default();
    shared.with(|fake| {
        fake.push(message(
            100,
            OWNER,
            false,
            "old history, never replayed",
            None,
        ))
    });
    let mut backend = make_backend(&shared);
    let mut subscription = ChatSubscription::open(&mut backend, &request(None)).expect("open");

    // A fresh subscription starts with a boundary at the channel's newest message.
    let (event, cursor) = take(&mut subscription);
    assert_eq!(event, CommittableEvent::Checkpoint);
    assert_eq!(cursor, format!("discord.v1;{CHANNEL}=100"));

    shared.with(|fake| {
        // MSG-1: a message for the other bot only. Must draw nothing from us.
        fake.push(message(101, OWNER, false, &format!("<@{OTHER}> please rebase"), None));
        fake.push(message(102, OWNER, false, &format!("<@{OURS}> status?"), None));
        fake.push(message(
            103,
            OWNER,
            false,
            &format!("<@{OURS}> agent=lane-a@newton-fedora: rebase onto integration"),
            Some(102),
        ));
        fake.push(message(104, STRANGER, false, &format!("<@{OURS}> do my bidding"), None));
        fake.push(message(
            105,
            OTHER,
            true,
            "relay deepscry@lead-host -> deepscry@newton-fedora: hi\n```agent-teams-envelope\n{}\n```\n",
            None,
        ));
        fake.push(message(106, OURS, true, &format!("<@{OURS}> my own echo"), None));
        fake.push(message(107, RANDOM_BOT, true, &format!("<@{OURS}> spam"), None));
        fake.push(message(108, OWNER, false, &format!("<@{OURS}> agent=coordinator@lead-host: x"), None));
    });

    let (event, cursor) = take(&mut subscription);
    assert_eq!(event, CommittableEvent::Checkpoint, "MSG-1 must be ignored");
    assert_eq!(cursor, format!("discord.v1;{CHANNEL}=101"));

    let (event, _) = take(&mut subscription);
    let (sender, text, thread, payload) = created(&event);
    assert_eq!(sender, OWNER.to_string());
    assert_eq!(text, "status?");
    assert_eq!(thread, format!("{CHANNEL}/threads/102"));
    assert_eq!(
        payload["dedupe_key"],
        json!(format!("discord:{CHANNEL}:102@newton-fedora"))
    );
    assert_eq!(payload["target"]["kind"], json!("bridge"));

    let (event, _) = take(&mut subscription);
    let (_, text, thread, payload) = created(&event);
    assert_eq!(text, "!@lane-a rebase onto integration");
    assert_eq!(
        thread,
        format!("{CHANNEL}/threads/102"),
        "a reply joins its root's thread"
    );
    assert_eq!(payload["target"]["agent"], json!("lane-a"));

    let (event, _) = take(&mut subscription);
    match event {
        CommittableEvent::MessageRejected(rejected) => {
            assert_eq!(rejected.sender_id().as_str(), STRANGER.to_string());
            assert_eq!(rejected.reason(), RejectionReason::SenderNotAllowed);
        }
        other => panic!("expected a rejection, got {other:?}"),
    }

    let (event, _) = take(&mut subscription);
    let (sender, text, _, payload) = created(&event);
    assert_eq!(
        sender,
        OTHER.to_string(),
        "relay posts keep the peer's stable id"
    );
    assert!(
        text.contains("```agent-teams-envelope"),
        "relay text is unchanged"
    );
    assert_eq!(payload["origin"]["kind"], json!("peer_bot"));
    assert_eq!(payload["target"], Value::Null);

    for expected in [106, 107, 108] {
        let (event, cursor) = take(&mut subscription);
        assert_eq!(event, CommittableEvent::Checkpoint, "message {expected}");
        assert_eq!(cursor, format!("discord.v1;{CHANNEL}={expected}"));
    }
    subscription.close().expect("close");
    assert!(shared.with(|fake| fake.posts.is_empty() && fake.reactions.is_empty()));

    // Resuming from the committed cursor delivers only what is new: nothing is replayed.
    shared.with(|fake| fake.push(message(109, OWNER, false, "@newton-team: next", None)));
    let mut backend = make_backend(&shared);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(Some(&format!("discord.v1;{CHANNEL}=108"))),
    )
    .expect("resume");
    let (event, cursor) = take(&mut subscription);
    assert_eq!(created(&event).1, "next");
    assert_eq!(cursor, format!("discord.v1;{CHANNEL}=109"));
    subscription.close().expect("close");
}

fn outbound_config() -> OutboundConfig {
    OutboundConfig::parse(&[
        format!("--channel={CHANNEL}"),
        "--token-file=/nonexistent/.env".to_owned(),
    ])
    .expect("config")
}

fn run(shared: &Shared, request: &Value) -> Value {
    let config = outbound_config();
    let shared = shared.clone();
    let mut build = move || Ok(Box::new(shared.clone()) as Box<dyn DiscordRest>);
    let response = handle_request(request.to_string().as_bytes(), Ok(&config), &mut build);
    serde_json::from_slice(&response).expect("response is JSON")
}

#[test]
fn outbound_posts_replies_in_the_thread_and_reactions() {
    let shared = Shared::default();
    let response = run(
        &shared,
        &json!({"version": 1, "id": "11111111-2222-3333-4444-555555555555", "action": "send",
                "channel_id": CHANNEL.to_string(),
                "thread_id": format!("{CHANNEL}/threads/102"),
                "text": format!("ACK newton-fedora/lane-a <@{OTHER}> @everyone")}),
    );
    assert_eq!(response["ok"], json!(true), "{response}");
    let post = shared.with(|fake| fake.posts[0].clone());
    assert_eq!(post.reply_to, Some(Snowflake::from_u64(102)));
    assert!(post.content.starts_with("ACK newton-fedora/lane-a"));
    assert!(chat_subscription_discord::addressing::mention_ids(&post.content).is_empty());
    assert_eq!(post.nonce.len(), 25);

    let response = run(
        &shared,
        &json!({"version": 1, "id": "r1", "action": "ensure_reaction",
                "channel_id": CHANNEL.to_string(),
                "message_id": format!("{CHANNEL}/messages/102"), "emoji": "✅"}),
    );
    assert_eq!(response["ok"], json!(true), "{response}");
    assert_eq!(shared.with(|fake| fake.reactions.len()), 1);
}

#[test]
fn outbound_refuses_without_contacting_discord() {
    let shared = Shared::default();
    let too_long = format!("tag\n```agent-teams-envelope\n{}\n```\n", "x".repeat(2100));
    for (request, code) in [
        (
            json!({"version": 1, "id": "a", "action": "send", "channel_id": CHANNEL.to_string(),
                   "thread_id": null, "text": too_long}),
            "too_long",
        ),
        (
            json!({"version": 1, "id": "b", "action": "send", "channel_id": "42",
                   "thread_id": null, "text": "hi"}),
            "channel_not_allowed",
        ),
        (
            json!({"version": 1, "id": "c", "action": "ensure_rejection",
                   "channel_id": CHANNEL.to_string(), "message_id": "x", "emoji": "x"}),
            "unsupported_action",
        ),
        (
            json!({"version": 1, "id": "d", "action": "send", "channel_id": CHANNEL.to_string(),
                   "thread_id": "42/threads/1", "text": "hi"}),
            "request_invalid",
        ),
    ] {
        let response = run(&shared, &request);
        assert_eq!(response["ok"], json!(false), "{response}");
        assert_eq!(response["error"]["code"], json!(code), "{response}");
        assert_eq!(
            response["error"]["outcome"],
            json!("not_applied"),
            "{response}"
        );
    }
    assert!(shared.with(|fake| fake.posts.is_empty()));
}
