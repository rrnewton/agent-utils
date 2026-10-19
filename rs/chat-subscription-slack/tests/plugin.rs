//! Inbound behaviour of the Slack subscription backend against the fake transport.

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use chat_subscription::{
    BackendConfiguration, ChannelId, ChatSubscription, ChatSubscriptionBackend, CommittableEvent,
    DeliveryBatch, InboundMessage, ProviderCursor, SenderId, SubscribeRequest, SubscriptionError,
    SubscriptionItem,
};
use chat_subscription_slack::fake::{FakeStep, FakeTransport};
use chat_subscription_slack::plugin::SlackBackend;
use chat_subscription_slack::transport::{Catchup, SlackTransport};
use serde_json::{json, Map, Value};

const CHANNEL: &str = "C0C91RW5FAR";
const OWNER: &str = "U0OWNER0001";
// 2026-10-14T18:00:00Z, the fake clock's "now".
const NOW_MILLIS: u64 = 1_792_000_800_000;

fn now() -> u64 {
    NOW_MILLIS
}

fn configuration(extra: Value) -> BackendConfiguration {
    let mut data: Map<String, Value> = Map::from_iter([
        ("cli_path".to_owned(), json!("/opt/slack/bin/slack-cli")),
        ("heartbeat_interval_seconds".to_owned(), json!(1)),
    ]);
    if let Value::Object(extra) = extra {
        data.extend(extra);
    }
    BackendConfiguration::new("agentctl.slack.v1", data).expect("configuration")
}

fn request(resume: Option<&str>, extra: Value) -> SubscribeRequest {
    SubscribeRequest::new(
        vec![ChannelId::new(CHANNEL).unwrap()],
        vec![SenderId::new(OWNER).unwrap()],
        resume.map(|cursor| ProviderCursor::new(cursor).unwrap()),
    )
    .unwrap()
    .with_backend_configuration(configuration(extra))
}

fn make_backend(fake: &FakeTransport) -> SlackBackend {
    let fake = fake.clone();
    SlackBackend::new(Box::new(move |_config| {
        Ok(Box::new(fake.clone()) as Box<dyn SlackTransport>)
    }))
    .with_clock(now)
}

fn event(sequence: u64, slack: Value) -> FakeStep {
    FakeStep::Line(
        json!({
            "seq": sequence, "ts": 1.0, "type": "message.created", "source": "slack",
            "iris_seq_id": sequence,
            "payload": {"event_type": "message.created", "container_id": CHANNEL, "slack_message": slack}
        })
        .to_string(),
    )
}

fn owner_message(sequence: u64, ts: &str, thread_ts: Option<&str>, text: &str) -> FakeStep {
    let mut slack =
        json!({"channel_id": CHANNEL, "message_ts": ts, "slack_user_id": OWNER, "text": text});
    if let Some(thread_ts) = thread_ts {
        slack["thread_ts"] = json!(thread_ts);
    }
    event(sequence, slack)
}

fn next_batch(subscription: &mut ChatSubscription) -> DeliveryBatch {
    loop {
        match subscription.next_item().expect("next item") {
            Some(SubscriptionItem::Batch(batch)) => return batch,
            Some(SubscriptionItem::Heartbeat(_)) => {}
            None => panic!("stream ended"),
        }
    }
}

fn commit_next(subscription: &mut ChatSubscription) -> DeliveryBatch {
    let batch = next_batch(subscription);
    subscription.commit_durable(&batch).expect("commit");
    batch
}

fn single_message(batch: &DeliveryBatch) -> &InboundMessage {
    match batch.events() {
        [CommittableEvent::MessageCreated(message)] => message,
        other => panic!("expected one message, found {other:?}"),
    }
}

#[test]
fn fresh_lineage_checkpoints_then_delivers_owner_messages_and_thread_replies() {
    let fake = FakeTransport::new();
    fake.push_script(vec![
        owner_message(101, "1792000801.000100", None, "please &amp; thank you"),
        // A stranger and a bot are checkpoints, not requests.
        event(102, json!({"channel_id": CHANNEL, "message_ts": "1792000802.000100", "slack_user_id": "U0STRANGER1", "text": "hi"})),
        event(103, json!({"channel_id": CHANNEL, "message_ts": "1792000803.000100", "slack_user_id": "U0BOTUSER01", "bot_id": "B0BOT000001", "text": "[bot] reply"})),
        // The owner replies inside the first message's thread.
        owner_message(104, "1792000804.000100", Some("1792000801.000100"), "follow-up"),
        // An edit notice is not a request.
        event(105, json!({"channel_id": CHANNEL, "message_ts": "1792000805.000100", "slack_user_id": OWNER, "subtype": "message_changed", "text": "x"})),
        // A message older than the lineage start is not delivered.
        owner_message(106, "1792000000.000100", None, "old"),
        FakeStep::Line(r#"{"type":"HEARTBEAT","ts":1.0}"#.to_owned()),
    ]);
    let mut backend = make_backend(&fake);
    let mut subscription = ChatSubscription::open(&mut backend, &request(None, json!({}))).unwrap();

    let first = commit_next(&mut subscription);
    assert_eq!(first.events(), [CommittableEvent::Checkpoint]);
    assert_eq!(
        first.cursor().as_str(),
        format!("slack1:since:{NOW_MILLIS}")
    );
    assert_eq!(fake.state().opened[0].catchup, Catchup::None);
    assert_eq!(fake.state().opened[0].channel_ids, vec![CHANNEL.to_owned()]);

    let top = commit_next(&mut subscription);
    assert_eq!(top.cursor().as_str(), "slack1:seq:101");
    let message = single_message(&top);
    assert_eq!(message.channel_id().as_str(), CHANNEL);
    assert_eq!(
        message.message_id().as_str(),
        "C0C91RW5FAR/messages/1792000801.000100"
    );
    assert_eq!(
        message.thread_id().as_str(),
        "C0C91RW5FAR/threads/1792000801.000100"
    );
    assert_eq!(message.sender_id().as_str(), OWNER);
    assert_eq!(message.text(), "please & thank you");
    assert!(!message.is_thread_reply());
    assert_eq!(message.created_at(), "2026-10-14T18:00:01.000100Z");
    assert_eq!(
        message.provider_payload().unwrap().schema(),
        "agentctl.slack.message.v1"
    );

    for expected in ["slack1:seq:102", "slack1:seq:103"] {
        let skipped = commit_next(&mut subscription);
        assert_eq!(skipped.cursor().as_str(), expected);
        assert_eq!(skipped.events(), [CommittableEvent::Checkpoint]);
    }

    let reply = commit_next(&mut subscription);
    let message = single_message(&reply);
    assert_eq!(
        message.thread_id().as_str(),
        "C0C91RW5FAR/threads/1792000801.000100"
    );
    assert_eq!(
        message.message_id().as_str(),
        "C0C91RW5FAR/messages/1792000804.000100"
    );
    assert!(message.is_thread_reply());

    for expected in ["slack1:seq:105", "slack1:seq:106"] {
        let skipped = commit_next(&mut subscription);
        assert_eq!(skipped.cursor().as_str(), expected);
        assert_eq!(skipped.events(), [CommittableEvent::Checkpoint]);
    }
    assert!(matches!(
        subscription.next_item().unwrap(),
        Some(SubscriptionItem::Heartbeat(_))
    ));
    subscription.close().unwrap();
}

#[test]
fn restart_resumes_from_the_saved_sequence_and_skips_committed_events() {
    let fake = FakeTransport::new();
    fake.push_script(vec![
        owner_message(7, "1792000801.000100", None, "already admitted"),
        owner_message(8, "1792000802.000100", None, "new after restart"),
    ]);
    let mut backend = make_backend(&fake);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(Some("slack1:seq:7"), json!({}))).unwrap();
    let batch = commit_next(&mut subscription);
    assert_eq!(batch.cursor().as_str(), "slack1:seq:8");
    assert_eq!(single_message(&batch).text(), "new after restart");
    assert_eq!(fake.state().opened[0].catchup, Catchup::Sequence(7));
}

#[test]
fn restart_before_any_event_replays_from_the_lineage_start() {
    let fake = FakeTransport::new();
    fake.push_script(vec![owner_message(
        3,
        "1792000790.000100",
        None,
        "sent while down",
    )]);
    let mut backend = make_backend(&fake);
    let since = NOW_MILLIS - 60_000;
    let cursor = format!("slack1:since:{since}");
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(Some(&cursor), json!({}))).unwrap();
    let batch = commit_next(&mut subscription);
    assert_eq!(single_message(&batch).text(), "sent while down");
    assert_eq!(fake.state().opened[0].catchup, Catchup::SinceSeconds(65));
}

#[test]
fn identical_replay_after_a_host_rollback() {
    // The host rolled back its admission of sequence 9 and resumes from 8: the replayed event
    // must form a byte-identical batch.
    let script = vec![owner_message(
        9,
        "1792000801.000100",
        Some("1792000700.000100"),
        "same",
    )];
    let mut batches = Vec::new();
    for _ in 0..2 {
        let fake = FakeTransport::new();
        fake.push_script(script.clone());
        let mut backend = make_backend(&fake);
        let mut subscription =
            ChatSubscription::open(&mut backend, &request(Some("slack1:seq:8"), json!({})))
                .unwrap();
        batches.push(next_batch(&mut subscription));
    }
    assert_eq!(batches[0].cursor(), batches[1].cursor());
    assert_eq!(batches[0].events(), batches[1].events());
}

#[test]
fn provider_gaps_and_unreadable_cursors_are_reported_as_gaps() {
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Line(
        r#"{"type":"GAP","lost_from_iris_seq_id":10,"lost_to_iris_seq_id":20,"reason":"retention_overflow"}"#
            .to_owned(),
    )]);
    let mut backend = make_backend(&fake);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(Some("slack1:seq:9"), json!({}))).unwrap();
    let gap = next_batch(&mut subscription);
    assert!(gap.events()[0].requires_reconciliation());
    assert_eq!(gap.cursor().as_str(), "slack1:seq:9");

    let fake = FakeTransport::new();
    let mut second = make_backend(&fake);
    let mut subscription =
        ChatSubscription::open(&mut second, &request(Some("18230"), json!({}))).unwrap();
    let gap = next_batch(&mut subscription);
    assert!(gap.events()[0].requires_reconciliation());
}

#[test]
fn an_allowed_sender_posting_through_an_app_is_admitted_and_other_bots_are_not() {
    // An app acting for the owner (the owner's user token) is attributed to
    // the owner but stamped with the delivering app's bot_id and app_id.
    let log = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("slack-plugin-via-app.log");
    let _ = fs::remove_file(&log);
    let via_app = |sequence, ts: &str, user: &str, text: &str| {
        event(
            sequence,
            json!({"channel_id": CHANNEL, "message_ts": ts, "thread_ts": "1792000801.000100",
                "slack_user_id": user, "bot_id": "B0VIAAPP001", "app_id": "A0VIAAPP001", "text": text}),
        )
    };
    let fake = FakeTransport::new();
    fake.push_script(vec![
        via_app(21, "1792000802.000100", OWNER, "sent from another app"),
        // The bridge's own reply comes from its bot user, not an allowed sender.
        via_app(22, "1792000803.000100", "U0BRIDGEBOT1", "a bridge reply"),
        // The owner's message that only names the delivering app.
        event(
            23,
            json!({"channel_id": CHANNEL, "message_ts": "1792000804.000100",
            "slack_user_id": OWNER, "app_id": "A0VIAAPP001", "text": "typed in Slack"}),
        ),
        // Ignored prefixes still apply to the owner's posts through an app.
        via_app(24, "1792000805.000100", OWNER, "[coordinator] echo"),
    ]);
    let mut backend = make_backend(&fake);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(
            Some("slack1:seq:20"),
            json!({"ignore_text_prefixes": ["[coordinator]"], "diagnostics_log": log}),
        ),
    )
    .unwrap();

    let admitted = commit_next(&mut subscription);
    let message = single_message(&admitted);
    assert_eq!(message.sender_id().as_str(), OWNER);
    assert_eq!(message.text(), "sent from another app");
    assert!(message.is_thread_reply());

    let bot = commit_next(&mut subscription);
    assert_eq!(bot.cursor().as_str(), "slack1:seq:22");
    assert_eq!(bot.events(), [CommittableEvent::Checkpoint]);

    assert_eq!(
        single_message(&commit_next(&mut subscription)).text(),
        "typed in Slack"
    );

    let echo = commit_next(&mut subscription);
    assert_eq!(echo.cursor().as_str(), "slack1:seq:24");
    assert_eq!(echo.events(), [CommittableEvent::Checkpoint]);

    let diagnostics = fs::read_to_string(&log).unwrap();
    assert!(diagnostics.contains("event 22: checkpoint (bot author; user U0BRIDGEBOT1)"));
    assert!(diagnostics.contains("event 24: checkpoint (ignored prefix; user U0OWNER0001)"));
}

#[test]
fn ignored_prefixes_and_a_closed_stream() {
    let fake = FakeTransport::new();
    fake.push_script(vec![
        owner_message(11, "1792000801.000100", None, "  [coordinator] my own echo"),
        FakeStep::Close("tail exited".to_owned()),
    ]);
    let mut backend = make_backend(&fake);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(
            Some("slack1:seq:10"),
            json!({"ignore_text_prefixes": ["[coordinator]"]}),
        ),
    )
    .unwrap();
    let batch = commit_next(&mut subscription);
    assert_eq!(batch.events(), [CommittableEvent::Checkpoint]);
    match subscription.next_item() {
        Err(SubscriptionError::Backend(failure)) => {
            assert_eq!(failure.code(), "stream_closed");
            assert!(failure.retryable());
        }
        other => panic!("expected a retryable stream failure, found {other:?}"),
    }
}

#[test]
fn cancellation_wakes_a_blocked_receive() {
    let fake = FakeTransport::new();
    fake.push_script(Vec::new());
    let mut backend = make_backend(&fake);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(
            Some("slack1:seq:1"),
            json!({"heartbeat_interval_seconds": 300}),
        ),
    )
    .unwrap();
    let cancellation = subscription.cancellation();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        cancellation.cancel().unwrap();
    });
    let started = std::time::Instant::now();
    assert!(subscription.next_item().is_err());
    assert!(started.elapsed() < Duration::from_secs(30));
    canceller.join().unwrap();
}

#[test]
fn configuration_is_validated() {
    let fake = FakeTransport::new();
    let mut backend = make_backend(&fake);
    for bad in [
        json!({"unknown_key": 1}),
        json!({"cli_path": "relative/slack"}),
        json!({"transport": "smoke-signals"}),
    ] {
        let error = backend
            .subscribe(&request(None, bad))
            .err()
            .expect("refused");
        assert_eq!(error.code(), "slack_configuration_invalid");
        assert!(!error.retryable());
    }
    let wrong_sender = SubscribeRequest::new(
        vec![ChannelId::new(CHANNEL).unwrap()],
        vec![SenderId::new("users/123").unwrap()],
        None,
    )
    .unwrap()
    .with_backend_configuration(configuration(json!({})));
    assert!(backend.subscribe(&wrong_sender).is_err());
}
