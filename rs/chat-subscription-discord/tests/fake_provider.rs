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
    fn get_message(
        &mut self,
        channel: Snowflake,
        message: Snowflake,
    ) -> Result<DiscordMessage, RestError> {
        self.with(|fake| fake.get_message(channel, message))
    }
}

fn message(
    id: u64,
    author: u64,
    bot: bool,
    content: &str,
    reply_to: Option<u64>,
) -> DiscordMessage {
    let mut message = DiscordMessage::new(
        Snowflake::from_u64(id),
        Snowflake::from_u64(CHANNEL),
        Snowflake::from_u64(author),
        content,
        reply_to.map(Snowflake::from_u64),
    );
    message.author_is_bot = bot;
    message
}

fn request(resume: Option<&str>) -> SubscribeRequest {
    request_with(resume, &json!({}))
}

fn request_with(resume: Option<&str>, extra: &Value) -> SubscribeRequest {
    let mut data = json!({
        "bot_user_id": OURS.to_string(),
        "host": "newton-fedora",
        "aliases": ["newton-team"],
        "local_agents": ["coordinator", "lane-a"],
        "peer_bots": {OTHER.to_string(): "fleet@lead-host"},
        "token_file": "/nonexistent/.env",
        "poll_seconds": 2,
        "heartbeat_seconds": 5
    });
    for (key, value) in extra.as_object().expect("extra settings") {
        data[key] = value.clone();
    }
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
        fake.push(message(
            101,
            OWNER,
            false,
            &format!("<@{OTHER}> please rebase"),
            None,
        ));
        fake.push(message(
            102,
            OWNER,
            false,
            &format!("<@{OURS}> status?"),
            None,
        ));
        fake.push(message(
            103,
            OWNER,
            false,
            &format!("<@{OURS}> agent=lane-a@newton-fedora: rebase onto integration"),
            Some(102),
        ));
        fake.push(message(
            104,
            STRANGER,
            false,
            &format!("<@{OURS}> do my bidding"),
            None,
        ));
        fake.push(message(
            105,
            OTHER,
            true,
            "relay fleet@lead-host -> fleet@newton-fedora: hi\n```agent-teams-envelope\n{}\n```\n",
            None,
        ));
        fake.push(message(
            106,
            OURS,
            true,
            &format!("<@{OURS}> my own echo"),
            None,
        ));
        fake.push(message(
            107,
            RANDOM_BOT,
            true,
            &format!("<@{OURS}> spam"),
            None,
        ));
        fake.push(message(
            108,
            OWNER,
            false,
            &format!("<@{OURS}> agent=coordinator@lead-host: x"),
            None,
        ));
        // The peer's ordinary chat reply is not relay traffic: relay-in would quarantine it.
        fake.push(message(
            109,
            OTHER,
            true,
            "Bound to lane-b. Messages in this thread go to it.",
            None,
        ));
        let mut webhook = message(
            110,
            424_242,
            false,
            &format!("<@{OURS}> from a webhook"),
            None,
        );
        webhook.webhook = true;
        fake.push(webhook);
        let mut notice = message(
            111,
            OWNER,
            false,
            &format!("<@{OURS}> pinned a message"),
            None,
        );
        notice.kind = 6;
        fake.push(notice);
        fake.push(message(
            112,
            OWNER,
            false,
            &format!("<@{OURS}> agent=nobody@newton-fedora: x"),
            None,
        ));
        fake.push(message(
            113,
            STRANGER,
            false,
            &format!("<@{OURS}> agent=nobody@newton-fedora: x"),
            None,
        ));
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

    for expected in [106, 107] {
        let (event, cursor) = take(&mut subscription);
        assert_eq!(event, CommittableEvent::Checkpoint, "message {expected}");
        assert_eq!(cursor, format!("discord.v1;{CHANNEL}={expected}"));
    }
    // An owner's refused target gets feedback: another host when only this bot was addressed
    // (108), an agent this host does not have (112). A stranger's refused target is a sender
    // rejection (113): authorization comes first.
    for (expected, sender, reason) in [
        (108, OWNER, RejectionReason::TargetRefused),
        (109, 0, RejectionReason::TargetRefused),
        (110, 0, RejectionReason::TargetRefused),
        (111, 0, RejectionReason::TargetRefused),
        (112, OWNER, RejectionReason::TargetRefused),
        (113, STRANGER, RejectionReason::SenderNotAllowed),
    ] {
        let (event, cursor) = take(&mut subscription);
        assert_eq!(cursor, format!("discord.v1;{CHANNEL}={expected}"));
        if sender == 0 {
            assert_eq!(event, CommittableEvent::Checkpoint, "message {expected}");
            continue;
        }
        match event {
            CommittableEvent::MessageRejected(rejected) => {
                assert_eq!(rejected.sender_id().as_str(), sender.to_string());
                assert_eq!(rejected.reason(), reason, "message {expected}");
            }
            other => panic!("message {expected}: expected a rejection, got {other:?}"),
        }
    }
    subscription.close().expect("close");
    assert!(shared.with(|fake| fake.posts.is_empty() && fake.reactions.is_empty()));

    // Resuming from the committed cursor delivers only what is new: nothing is replayed.
    shared.with(|fake| fake.push(message(114, OWNER, false, "@newton-team: next", None)));
    let mut backend = make_backend(&shared);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(Some(&format!("discord.v1;{CHANNEL}=113"))),
    )
    .expect("resume");
    let (event, cursor) = take(&mut subscription);
    assert_eq!(created(&event).1, "next");
    assert_eq!(cursor, format!("discord.v1;{CHANNEL}=114"));
    subscription.close().expect("close");
}

#[test]
fn a_backlog_over_one_page_is_delivered_in_order_with_one_list_call_per_page() {
    let shared = Shared::default();
    shared.with(|fake| fake.push(message(1_000, OWNER, false, "history", None)));
    let mut backend = make_backend(&shared);
    let mut subscription = ChatSubscription::open(&mut backend, &request(None)).expect("open");
    assert_eq!(take(&mut subscription).0, CommittableEvent::Checkpoint);
    assert_eq!(
        shared.with(|fake| fake.list_calls),
        1,
        "the boundary is one call"
    );
    shared.with(|fake| {
        for n in 1..=150 {
            fake.push(message(
                1_000 + n,
                OWNER,
                false,
                &format!("<@{OURS}> item {n}"),
                None,
            ));
        }
    });
    for n in 1..=150 {
        let (event, cursor) = take(&mut subscription);
        assert_eq!(created(&event).1, format!("item {n}"));
        assert_eq!(cursor, format!("discord.v1;{CHANNEL}={}", 1_000 + n));
    }
    assert_eq!(
        shared.with(|fake| fake.list_calls),
        3,
        "one list call per page of 100, however many posts it returns"
    );
    subscription.close().expect("close");
}

#[test]
fn a_restart_mid_thread_keeps_the_reply_chain_on_its_root() {
    let shared = Shared::default();
    shared.with(|fake| fake.push(message(100, OWNER, false, "history", None)));
    let mut backend = make_backend(&shared);
    let mut subscription = ChatSubscription::open(&mut backend, &request(None)).expect("open");
    take(&mut subscription);
    shared.with(|fake| {
        fake.push(message(
            200,
            OWNER,
            false,
            &format!("<@{OURS}> start"),
            None,
        ));
        fake.push(message(201, OURS, true, "reply from this bot", Some(200)));
    });
    let (event, _) = take(&mut subscription);
    assert_eq!(created(&event).2, format!("{CHANNEL}/threads/200"));
    let (_, cursor) = take(&mut subscription);
    subscription.close().expect("close");

    // A fresh process has no root memory; the owner answers the bot's reply.
    shared.with(|fake| {
        fake.push(message(
            202,
            OWNER,
            false,
            &format!("<@{OURS}> follow-up"),
            Some(201),
        ))
    });
    let mut backend = make_backend(&shared);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(Some(&cursor))).expect("resume");
    let (event, _) = take(&mut subscription);
    assert_eq!(
        created(&event).2,
        format!("{CHANNEL}/threads/200"),
        "the root is read back from Discord, not taken to be the direct parent"
    );
    subscription.close().expect("close");
}

#[test]
fn an_unparsable_message_and_a_rate_limited_baseline_do_not_end_the_subscription() {
    let shared = Shared::default();
    shared.with(|fake| {
        fake.push(message(100, OWNER, false, "history", None));
        fake.fail_next.push_back(RestError::from_status(
            429,
            r#"{"retry_after": 0.1, "global": false}"#,
        ));
    });
    let mut backend = make_backend(&shared);
    let mut subscription = ChatSubscription::open(&mut backend, &request(None)).expect("open");
    let (event, cursor) = take(&mut subscription);
    assert_eq!(event, CommittableEvent::Checkpoint, "the baseline retried");
    assert_eq!(cursor, format!("discord.v1;{CHANNEL}=100"));
    assert_eq!(shared.with(|fake| fake.list_calls), 2);
    shared.with(|fake| {
        let mut broken = message(101, OWNER, false, &format!("<@{OURS}> lost"), None);
        broken.kind = chat_subscription_discord::transport::UNPARSABLE_MESSAGE_TYPE;
        fake.push(broken);
        fake.push(message(102, OWNER, false, &format!("<@{OURS}> next"), None));
    });
    let (event, cursor) = take(&mut subscription);
    assert_eq!(event, CommittableEvent::Checkpoint, "skipped, not fatal");
    assert_eq!(
        cursor,
        format!("discord.v1;{CHANNEL}=101"),
        "the cursor moves past it"
    );
    let (event, _) = take(&mut subscription);
    assert_eq!(created(&event).1, "next");
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
    run_with(shared, request, &outbound_config())
}

fn run_with(shared: &Shared, request: &Value, config: &OutboundConfig) -> Value {
    let config = config.clone();
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
                   "channel_id": CHANNEL.to_string(),
                   "message_id": format!("{CHANNEL}/messages/1"),
                   "sender_id": OWNER.to_string(), "emoji": "❌"}),
            "helper_misconfigured",
        ),
        (
            json!({"version": 1, "id": "c2", "action": "update", "channel_id": CHANNEL.to_string()}),
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
    assert!(shared.with(|fake| fake.posts.is_empty() && fake.calls == 0));
}

struct Ledger(std::path::PathBuf);

impl Ledger {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("discord-ledger-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }

    fn config(&self) -> OutboundConfig {
        OutboundConfig::parse(&[
            format!("--channel={CHANNEL}"),
            "--token-file=/nonexistent/.env".to_owned(),
            format!("--ledger={}", self.0.display()),
        ])
        .expect("config")
    }
}

impl Drop for Ledger {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rejection(id: &str, message: u64, sender: u64, emoji: &str) -> Value {
    json!({"version": 1, "id": id, "action": "ensure_rejection",
           "channel_id": CHANNEL.to_string(),
           "message_id": format!("{CHANNEL}/messages/{message}"),
           "sender_id": sender.to_string(), "emoji": emoji})
}

#[test]
fn ensure_rejection_reacts_only_to_a_verified_person_and_replays_its_receipt() {
    let ledger = Ledger::new("human");
    let config = ledger.config();
    let shared = Shared::default();
    shared.with(|fake| {
        fake.push(message(201, STRANGER, false, "<@1> do my bidding", None));
        fake.push(message(202, RANDOM_BOT, true, "<@1> spam", None));
        let mut webhook = message(203, 424_242, false, "<@1> hook", None);
        webhook.webhook = true;
        fake.push(webhook);
    });

    let id = "123e4567-e89b-42d3-a456-426614174001";
    let response = run_with(&shared, &rejection(id, 201, STRANGER, "❌"), &config);
    assert_eq!(response["ok"], json!(true), "{response}");
    assert_eq!(
        response["receipt"],
        json!({"reaction_id": format!("{CHANNEL}/messages/201/reactions/❌"), "already_present": false})
    );
    assert_eq!(shared.with(|fake| fake.reactions.len()), 1);

    // A retry of the finished operation replays its receipt and contacts nothing, even after
    // the configured emoji changed.
    let calls = shared.with(|fake| fake.calls);
    let response = run_with(&shared, &rejection(id, 201, STRANGER, "🚫"), &config);
    assert_eq!(
        response["receipt"]["already_present"],
        json!(true),
        "{response}"
    );
    assert_eq!(
        response["receipt"]["reaction_id"],
        json!(format!("{CHANNEL}/messages/201/reactions/❌"))
    );
    assert_eq!(shared.with(|fake| fake.calls), calls);

    // The same id may not be reused for another message or sender.
    let response = run_with(&shared, &rejection(id, 202, RANDOM_BOT, "❌"), &config);
    assert_eq!(
        response["error"]["code"],
        json!("operation_conflict"),
        "{response}"
    );

    // Bots, webhooks, a sender that is not the author, and a deleted message get nothing.
    for (index, (message, sender)) in [
        (202, RANDOM_BOT),
        (203, 424_242),
        (201, OWNER),
        (299, OWNER),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("00000000-0000-4000-8000-00000000000{index}");
        let response = run_with(&shared, &rejection(&id, message, sender, "❌"), &config);
        assert_eq!(response["ok"], json!(false), "{response}");
        assert_eq!(
            response["error"]["code"],
            json!("author_not_human"),
            "{response}"
        );
        assert_eq!(
            response["error"]["outcome"],
            json!("not_applied"),
            "{response}"
        );
        assert_eq!(response["error"]["retryable"], json!(false), "{response}");
        // The refusal is recorded: a retry does not look the author up again.
        let calls = shared.with(|fake| fake.calls);
        let again = run_with(&shared, &rejection(&id, message, sender, "❌"), &config);
        assert_eq!(again["error"]["code"], json!("author_not_human"), "{again}");
        assert_eq!(shared.with(|fake| fake.calls), calls);
    }
    assert_eq!(shared.with(|fake| fake.reactions.len()), 1);
}

#[test]
fn an_interrupted_rejection_reports_unknown_and_keeps_its_first_emoji() {
    let ledger = Ledger::new("uncertain");
    let config = ledger.config();
    let shared = Shared::default();
    shared.with(|fake| fake.push(message(301, STRANGER, false, "hello", None)));
    let id = "123e4567-e89b-42d3-a456-426614174002";
    // The author check passes, then the reaction call fails with a server error: it may have
    // been applied.
    struct FailReaction(Shared);
    impl DiscordRest for FailReaction {
        fn list_messages(
            &mut self,
            channel: Snowflake,
            after: Option<Snowflake>,
            limit: u8,
        ) -> Result<Vec<DiscordMessage>, RestError> {
            self.0.list_messages(channel, after, limit)
        }
        fn create_message(
            &mut self,
            channel: Snowflake,
            content: &str,
            reply_to: Option<Snowflake>,
            nonce: &str,
        ) -> Result<Snowflake, RestError> {
            self.0.create_message(channel, content, reply_to, nonce)
        }
        fn add_reaction(
            &mut self,
            _channel: Snowflake,
            _message: Snowflake,
            _emoji: &str,
        ) -> Result<(), RestError> {
            Err(RestError::from_status(502, "{}"))
        }
        fn get_message(
            &mut self,
            channel: Snowflake,
            message: Snowflake,
        ) -> Result<DiscordMessage, RestError> {
            self.0.get_message(channel, message)
        }
    }
    let inner = shared.clone();
    let mut build = move || Ok(Box::new(FailReaction(inner.clone())) as Box<dyn DiscordRest>);
    let response = handle_request(
        rejection(id, 301, STRANGER, "❌").to_string().as_bytes(),
        Ok(&config),
        &mut build,
    );
    let response: Value = serde_json::from_slice(&response).expect("json");
    assert_eq!(response["error"]["outcome"], json!("unknown"), "{response}");

    // The message is then deleted; the retry cannot verify the author, and because the first
    // attempt may have reacted, the outcome stays unknown rather than not_applied.
    shared.with(|fake| fake.channels.clear());
    let response = run_with(&shared, &rejection(id, 301, STRANGER, "🚫"), &config);
    assert_eq!(
        response["error"]["code"],
        json!("author_not_human"),
        "{response}"
    );
    assert_eq!(response["error"]["outcome"], json!("unknown"), "{response}");

    // Restored, the retry completes with the FIRST emoji, not the newly configured one.
    shared.with(|fake| fake.push(message(301, STRANGER, false, "hello", None)));
    let response = run_with(&shared, &rejection(id, 301, STRANGER, "🚫"), &config);
    assert_eq!(response["ok"], json!(true), "{response}");
    assert_eq!(
        shared.with(|fake| fake.reactions.clone()),
        [(
            Snowflake::from_u64(CHANNEL),
            Snowflake::from_u64(301),
            "❌".to_owned()
        )]
    );
}

#[test]
fn outbound_waits_out_a_short_rate_limit_and_reports_a_long_one() {
    let shared = Shared::default();
    let send = |id: &str| {
        json!({"version": 1, "id": id, "action": "send", "channel_id": CHANNEL.to_string(),
               "thread_id": null, "text": "hi"})
    };
    shared.with(|fake| {
        fake.fail_next.push_back(RestError::from_status(
            429,
            r#"{"message": "You are being rate limited.", "retry_after": 0.02, "global": false}"#,
        ))
    });
    let response = run(&shared, &send("short-wait"));
    assert_eq!(response["ok"], json!(true), "{response}");
    let posts = shared.with(|fake| fake.posts.clone());
    assert_eq!(posts.len(), 1, "posted once, after the wait");
    assert_eq!(
        posts[0].nonce,
        chat_subscription_discord::outbound::nonce("short-wait", 0)
    );

    shared.with(|fake| {
        fake.fail_next.push_back(RestError::from_status(
            429,
            r#"{"retry_after": 60, "global": true}"#,
        ))
    });
    let response = run(&shared, &send("long-wait"));
    assert_eq!(
        response["error"]["code"],
        json!("rate_limited"),
        "{response}"
    );
    assert_eq!(
        response["error"]["outcome"],
        json!("not_applied"),
        "{response}"
    );
    assert_eq!(response["error"]["retryable"], json!(true), "{response}");
    assert_eq!(shared.with(|fake| fake.posts.len()), 1);
}

fn ack_config() -> OutboundConfig {
    OutboundConfig::parse(&[
        format!("--channel={CHANNEL}"),
        "--token-file=/nonexistent/.env".to_owned(),
        "--ack-host=newton-fedora".to_owned(),
        format!("--ack-bot-user-id={OURS}"),
        "--ack-alias=newton-team".to_owned(),
        "--ack-bridge-agent=coordinator".to_owned(),
    ])
    .expect("config")
}

fn react(id: &str, message: u64, emoji: &str) -> Value {
    json!({"version": 1, "id": id, "action": "ensure_reaction",
           "channel_id": CHANNEL.to_string(),
           "message_id": format!("{CHANNEL}/messages/{message}"), "emoji": emoji})
}

#[test]
fn the_ack_line_follows_the_bridge_acknowledgement_and_names_host_and_agent() {
    let shared = Shared::default();
    shared.with(|fake| {
        fake.push(message(
            401,
            OWNER,
            false,
            &format!("<@{OURS}> status?"),
            None,
        ));
        fake.push(message(
            402,
            OWNER,
            false,
            &format!("<@{OURS}> agent=lane-a@newton-fedora: rebase"),
            None,
        ));
        fake.push(message(
            403,
            OWNER,
            false,
            &format!("<@{OURS}> and this"),
            Some(401),
        ));
        fake.push(message(
            404,
            OTHER,
            true,
            "```agent-teams-envelope\n{}\n```",
            None,
        ));
    });
    let config = ack_config();
    for (index, target) in [401, 402, 403, 404].into_iter().enumerate() {
        let response = run_with(
            &shared,
            &react(&format!("ack-{index}"), target, "👀"),
            &config,
        );
        assert_eq!(response["ok"], json!(true), "{response}");
    }
    // The receipt reaction is not the acknowledgement: no line.
    run_with(&shared, &react("receipt", 401, "✅"), &config);
    let posts = shared.with(|fake| fake.posts.clone());
    let lines: Vec<(&str, Option<u64>, &str)> = posts
        .iter()
        .map(|post| {
            (
                post.content.as_str(),
                post.reply_to.map(Snowflake::get),
                post.nonce.as_str(),
            )
        })
        .collect();
    assert_eq!(
        lines,
        [
            (
                "ACK newton-fedora/coordinator: received",
                Some(401),
                "ack401"
            ),
            ("ACK newton-fedora/lane-a: received", Some(402), "ack402"),
            ("ACK newton-fedora: received", Some(403), "ack403"),
        ],
        "one line per acknowledged owner request; none for a relay post or a receipt"
    );
    assert_eq!(shared.with(|fake| fake.reactions.len()), 5);

    // Without --ack-host no line is ever posted, and a failed line never fails the reaction.
    let before = shared.with(|fake| fake.posts.len());
    run(&shared, &react("plain", 401, "👀"));
    assert_eq!(shared.with(|fake| fake.posts.len()), before);
    // The request cannot be read back (deleted): the line is skipped, the reaction succeeds.
    let response = run_with(&shared, &react("gone", 499, "👀"), &config);
    assert_eq!(response["ok"], json!(true), "{response}");
    assert_eq!(shared.with(|fake| fake.posts.len()), before);
}

#[test]
fn a_long_reply_with_an_ordinary_fence_is_posted_in_balanced_parts() {
    let shared = Shared::default();
    let code: String = (0..150).map(|n| format!("let v{n} = {n};\n")).collect();
    let text = format!("Patch:\n```rust\n{code}```\nDone.\n");
    let response = run(
        &shared,
        &json!({"version": 1, "id": "11111111-2222-3333-4444-666666666666", "action": "send",
                "channel_id": CHANNEL.to_string(), "thread_id": format!("{CHANNEL}/threads/7"),
                "text": text}),
    );
    assert_eq!(response["ok"], json!(true), "{response}");
    let posts = shared.with(|fake| fake.posts.clone());
    assert!(posts.len() > 1);
    for post in &posts {
        assert!(post.content.chars().count() <= 2000);
        assert_eq!(
            post.content.matches("```").count() % 2,
            0,
            "{}",
            post.content
        );
    }
    // The first part replies to the thread root; later parts reply to the first part, which
    // the plugin resolves back to the same root.
    assert_eq!(posts[0].reply_to, Some(Snowflake::from_u64(7)));
    assert!(posts[1..]
        .iter()
        .all(|post| post.reply_to == Some(Snowflake::from_u64(1_000_001))));
    assert!(posts[1].content.starts_with("```rust\n"));
}

#[test]
fn the_shipped_example_configures_both_the_plugin_and_the_helper() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/bridge-newton-fedora.json"),
    )
    .expect("example")
    .replace("<OWNER_CHANNEL_ID>", "1600000000000000001")
    .replace("<OWNER_DISCORD_USER_ID>", "111222333444555666")
    .replace("<PEER_HOST>", "peer-host")
    .replace("<TOKEN_FILE>", "/nonexistent/discord.env");
    let example: Value = serde_json::from_str(&text).expect("json");
    let backend = &example["backend_configuration"];
    let configuration = BackendConfiguration::new(
        backend["schema"].as_str().expect("schema"),
        backend["data"].as_object().cloned().expect("data"),
    )
    .expect("configuration");
    chat_subscription_discord::plugin::DiscordConfig::from_backend_configuration(&configuration)
        .expect("the plugin accepts the example");
    let arguments: Vec<String> = example["outbound_command"]["arguments"]
        .as_array()
        .expect("arguments")
        .iter()
        .map(|argument| argument.as_str().expect("string").to_owned())
        .collect();
    let helper = OutboundConfig::parse(&arguments).expect("the helper accepts the example");
    assert!(helper.ack.is_some() && helper.ledger.is_some());
    assert_eq!(helper.channels.len(), 2, "owner channel and relay channel");
}
