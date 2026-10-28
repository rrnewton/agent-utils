//! The periodic rescan (#485 chat-file-messages): messages the event stream never carries, such as
//! ones with files, reach the bridge through a history listing, once, across stream and rescan,
//! threads, edits and restarts.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use agentctl::chat_runtime::{consume_one, BridgeState, ConsumedItem};
use chat_subscription::{
    BackendConfiguration, ChannelId, ChatSubscription, CommittableEvent, DeliveryBatch,
    InboundMessage, ProviderCursor, SenderId, SubscribeRequest, SubscriptionItem,
};
use chat_subscription_slack::fake::{FakeStep, FakeTransport};
use chat_subscription_slack::plugin::SlackBackend;
use chat_subscription_slack::rescan::LEDGER_FILE;
use chat_subscription_slack::transport::{SlackMessage, SlackTransport, TransportError};
use serde_json::{json, Map, Value};

const CHANNEL: &str = "C0C91RW5FAR";
const OWNER: &str = "U0OWNER0001";
const BOT: &str = "U0BRIDGEBOT";
// 2026-10-14T18:00:00Z.
const T0: u64 = 1_792_000_800_000;

/// A test's own wall clock: tests run in parallel, so each gets its own static.
macro_rules! clock {
    ($name:ident) => {
        fn $name() -> u64 {
            static NOW: AtomicU64 = AtomicU64::new(T0);
            NOW.load(Ordering::SeqCst)
        }
    };
    ($name:ident, $set:ident) => {
        static $set: AtomicU64 = AtomicU64::new(T0);
        fn $name() -> u64 {
            $set.load(Ordering::SeqCst)
        }
    };
}

/// A private directory for one test's diagnostics log and rescan ledger.
fn scratch(name: &str) -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("slack-rescan-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

fn data(directory: &Path, extra: Value) -> Map<String, Value> {
    let mut data: Map<String, Value> = Map::from_iter([
        ("cli_path".to_owned(), json!("/opt/slack/bin/slack-cli")),
        ("heartbeat_interval_seconds".to_owned(), json!(1)),
        (
            "diagnostics_log".to_owned(),
            json!(directory.join("plugin.log").to_str().unwrap()),
        ),
    ]);
    if let Value::Object(extra) = extra {
        data.extend(extra);
    }
    data
}

fn request(directory: &Path, resume: Option<&str>, extra: Value) -> SubscribeRequest {
    SubscribeRequest::new(
        vec![ChannelId::new(CHANNEL).unwrap()],
        vec![SenderId::new(OWNER).unwrap()],
        resume.map(|cursor| ProviderCursor::new(cursor).unwrap()),
    )
    .unwrap()
    .with_backend_configuration(
        BackendConfiguration::new("agentctl.slack.v1", data(directory, extra)).unwrap(),
    )
}

fn backend(fake: &FakeTransport, clock: fn() -> u64) -> SlackBackend {
    let fake = fake.clone();
    SlackBackend::new(Box::new(move |_config| {
        Ok(Box::new(fake.clone()) as Box<dyn SlackTransport>)
    }))
    .with_clock(clock)
}

fn ts(millis: u64) -> String {
    format!("{}.{:06}", millis / 1_000, (millis % 1_000) * 1_000 + 100)
}

fn listed(
    ts: &str,
    thread_ts: Option<&str>,
    user: &str,
    text: &str,
    files: &[&str],
) -> SlackMessage {
    SlackMessage {
        channel_id: CHANNEL.to_owned(),
        message_ts: ts.to_owned(),
        thread_ts: thread_ts.map(str::to_owned),
        user_id: Some(user.to_owned()),
        text: text.to_owned(),
        file_ids: files.iter().map(|id| (*id).to_owned()).collect(),
        ..SlackMessage::default()
    }
}

fn stream_message(sequence: u64, ts: &str, thread_ts: Option<&str>, text: &str) -> FakeStep {
    let mut slack =
        json!({"channel_id": CHANNEL, "message_ts": ts, "slack_user_id": OWNER, "text": text});
    if let Some(thread_ts) = thread_ts {
        slack["thread_ts"] = json!(thread_ts);
    }
    FakeStep::Line(
        json!({
            "type": "message.created", "iris_seq_id": sequence,
            "payload": {"event_type": "message.created", "container_id": CHANNEL, "slack_message": slack}
        })
        .to_string(),
    )
}

/// Tail heartbeats, which the client writes only after its catch-up: each is an idle turn for
/// the plugin and tells it the stream has caught up.
fn heartbeats(count: usize) -> Vec<FakeStep> {
    vec![FakeStep::Line(r#"{"type":"HEARTBEAT","ts":1.0}"#.to_owned()); count]
}

/// The next batch, or `None` after `limit` heartbeats in a row.
fn next_batch_within(subscription: &mut ChatSubscription, limit: usize) -> Option<DeliveryBatch> {
    for _ in 0..limit {
        match subscription.next_item().expect("next item") {
            Some(SubscriptionItem::Batch(batch)) => return Some(batch),
            Some(SubscriptionItem::Heartbeat(_)) => {}
            None => panic!("stream ended"),
        }
    }
    None
}

fn commit_next(subscription: &mut ChatSubscription) -> DeliveryBatch {
    let batch = next_batch_within(subscription, 1).expect("a batch");
    subscription.commit_durable(&batch).expect("commit");
    batch
}

fn single_message(batch: &DeliveryBatch) -> &InboundMessage {
    match batch.events() {
        [CommittableEvent::MessageCreated(message)] => message,
        other => panic!("expected one message, found {other:?}"),
    }
}

fn ledger(directory: &Path) -> Value {
    serde_json::from_slice(&fs::read(directory.join(LEDGER_FILE)).unwrap()).unwrap()
}

fn log(directory: &Path) -> String {
    fs::read_to_string(directory.join("plugin.log")).unwrap_or_default()
}

clock!(file_clock, FILE_NOW);

#[test]
fn a_file_message_the_stream_never_carries_is_rescued_with_its_file_ids() {
    let directory = scratch("file");
    let fake = FakeTransport::new();
    let file_ts = ts(T0 + 10_000);
    let late_ts = ts(T0 + 100_000);
    // The stream carries nothing of the file message, then much later the same message
    // (a late duplicate) and a fresh one.
    let mut script = heartbeats(3);
    script.push(stream_message(41, &file_ts, None, "late copy"));
    script.push(stream_message(42, &late_ts, None, "plain"));
    fake.push_script(script);
    fake.push_history(listed(
        &file_ts,
        None,
        OWNER,
        "see &lt;this&gt;",
        &["F0CBYK33RNY"],
    ));
    fake.push_history(listed(&ts(T0 + 11_000), None, BOT, "bot reply", &[]));
    let mut backend = backend(&fake, file_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    assert_eq!(
        commit_next(&mut subscription).cursor().as_str(),
        format!("slack1:since:{T0}")
    );

    // At the start the message is not yet posted; 5 s after it was it is still the stream's.
    let first = next_batch_within(&mut subscription, 1);
    assert!(first.is_none());
    assert_eq!(fake.state().scans.len(), 1);
    FILE_NOW.store(T0 + 15_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(
        fake.state().scans.len(),
        1,
        "the next pass waits for the interval"
    );

    FILE_NOW.store(T0 + 95_000, Ordering::SeqCst);
    let rescued = next_batch_within(&mut subscription, 1).expect("rescued batch");
    let message = single_message(&rescued);
    assert_eq!(
        rescued.cursor().as_str(),
        format!("slack1:since:{T0};rescued:{CHANNEL}/{file_ts}")
    );
    assert_eq!(
        message.message_id().as_str(),
        format!("{CHANNEL}/messages/{file_ts}")
    );
    assert_eq!(
        message.thread_id().as_str(),
        format!("{CHANNEL}/threads/{file_ts}")
    );
    assert!(!message.is_thread_reply());
    assert!(message.text().starts_with(
        "see <this>\n\n[agentctl] This message reached the bridge through the periodic rescan"
    ));
    assert!(message
        .text()
        .contains("This message has 1 attached file: F0CBYK33RNY."));
    assert!(message.text().contains(&format!(
        "\n/opt/slack/bin/slack-cli slack.file download --file-id=F0CBYK33RNY --channel-id={CHANNEL} --output-path=/tmp/agentctl-slack-files/"
    )));
    let payload = message.provider_payload().unwrap().data();
    assert_eq!(payload["file_ids"], json!(["F0CBYK33RNY"]));
    assert_eq!(payload["delivered_by"], "rescan");
    // The pending record is durable before the bridge acknowledges.
    let saved = ledger(&directory);
    assert_eq!(saved["pending"]["cursor"], rescued.cursor().as_str());
    subscription.commit_durable(&rescued).unwrap();
    let saved = ledger(&directory);
    assert_eq!(saved["pending"], Value::Null);
    assert_eq!(saved["rescued"], json!([format!("{CHANNEL}/{file_ts}")]));

    // The stream's late copy is a checkpoint; the next message is delivered normally.
    let late_copy = commit_next(&mut subscription);
    assert_eq!(late_copy.cursor().as_str(), "slack1:seq:41");
    assert_eq!(late_copy.events(), [CommittableEvent::Checkpoint]);
    let plain = commit_next(&mut subscription);
    assert_eq!(single_message(&plain).text(), "plain");
    assert!(!single_message(&plain)
        .provider_payload()
        .unwrap()
        .data()
        .contains_key("delivered_by"));

    // Later passes offer nothing again.
    FILE_NOW.store(T0 + 400_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 3).is_none());
    assert!(fake.state().scans.len() >= 3);
    subscription.close().unwrap();
    let log = log(&directory);
    assert!(log.contains(&format!(
        "found 1 message the stream did not carry: {CHANNEL}/{file_ts} (1 file)"
    )));
    assert!(log.contains("was already delivered by the rescan"));
    fs::remove_dir_all(&directory).unwrap();
}

clock!(carried_clock, CARRIED_NOW);

#[test]
fn a_message_the_stream_carried_is_never_rescued_even_when_edited() {
    let directory = scratch("carried");
    let fake = FakeTransport::new();
    let carried = ts(T0 + 5_000);
    let mut script = vec![stream_message(7, &carried, None, "original")];
    script.extend(heartbeats(4));
    fake.push_script(script);
    // The listing shows the message edited since, with a file added.
    fake.push_history(listed(&carried, None, OWNER, "edited", &["F0AAAAAAAA1"]));
    let mut backend = backend(&fake, carried_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    commit_next(&mut subscription);
    assert_eq!(
        single_message(&commit_next(&mut subscription)).text(),
        "original"
    );
    CARRIED_NOW.store(T0 + 200_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 4).is_none());
    assert!(!fake.state().scans.is_empty());
    assert_eq!(
        ledger(&directory)["carried"],
        json!([format!("{CHANNEL}/{carried}")])
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(thread_clock, THREAD_NOW);

#[test]
fn a_reply_with_a_file_in_an_active_thread_is_rescued_across_pages() {
    let directory = scratch("thread");
    let fake = FakeTransport::new();
    fake.state().scan_page_size = 2;
    let root = ts(T0 + 1_000);
    let mut script = vec![stream_message(3, &root, None, "root")];
    script.extend(heartbeats(8));
    fake.push_script(script);
    fake.push_history(listed(&root, None, OWNER, "root", &[]));
    // Bot replies and an owner reply the stream dropped, past the first page of the thread.
    for (offset, user, files) in [
        (2_000, BOT, &[][..]),
        (3_000, BOT, &[][..]),
        (5_000, OWNER, &["F0BBBBBBBB1", "F0BBBBBBBB2"][..]),
    ] {
        fake.push_history(listed(&ts(T0 + offset), Some(&root), user, "", files));
    }
    let mut backend = backend(&fake, thread_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    commit_next(&mut subscription);
    commit_next(&mut subscription);
    THREAD_NOW.store(T0 + 100_000, Ordering::SeqCst);
    let rescued = next_batch_within(&mut subscription, 2).expect("rescued reply");
    let reply = single_message(&rescued);
    let reply_ts = ts(T0 + 5_000);
    assert_eq!(
        reply.message_id().as_str(),
        format!("{CHANNEL}/messages/{reply_ts}")
    );
    assert_eq!(
        reply.thread_id().as_str(),
        format!("{CHANNEL}/threads/{root}")
    );
    assert!(reply.is_thread_reply());
    assert!(reply
        .text()
        .contains("This message has 2 attached files: F0BBBBBBBB1, F0BBBBBBBB2."));
    assert!(reply.text().contains("--file-id=F0BBBBBBBB2"));
    subscription.commit_durable(&rescued).unwrap();
    let scans = fake.state().scans.clone();
    let thread_scans: Vec<_> = scans
        .iter()
        .filter(|scan| scan.1.as_deref() == Some(&root))
        .collect();
    assert_eq!(thread_scans.len(), 2, "two pages of the thread: {scans:?}");
    assert_eq!(thread_scans[1].3.as_deref(), Some("page-2"));

    // The next pass lists the thread no further: it has no newer reply.
    THREAD_NOW.store(T0 + 300_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 3).is_none());
    let later = fake.state().scans.len();
    assert!(fake.state().scans[scans.len()..later]
        .iter()
        .all(|scan| scan.1.is_none()));
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(grace_clock, GRACE_NOW);

#[test]
fn young_messages_wait_for_the_stream_and_old_ones_are_never_rescued() {
    let directory = scratch("grace");
    let fake = FakeTransport::new();
    fake.push_script(heartbeats(8));
    // Before the ledger began: an older message may have been carried before anything was
    // recorded, so it is never rescued.
    fake.push_history(listed(
        &ts(T0 - 30_000),
        None,
        OWNER,
        "before",
        &["F0CCCCCCCC1"],
    ));
    let young = ts(T0 + 85_000);
    fake.push_history(listed(&young, None, OWNER, "young", &[]));
    let mut backend = backend(&fake, grace_clock);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(&directory, None, json!({"initial_backfill_seconds": 600})),
    )
    .unwrap();
    commit_next(&mut subscription);
    GRACE_NOW.store(T0 + 95_000, Ordering::SeqCst);
    assert!(
        next_batch_within(&mut subscription, 1).is_none(),
        "10 s old is the stream's"
    );
    GRACE_NOW.store(T0 + 190_000, Ordering::SeqCst);
    let rescued = next_batch_within(&mut subscription, 1).expect("rescued once ripe");
    assert!(single_message(&rescued).text().starts_with("young"));
    subscription.commit_durable(&rescued).unwrap();
    GRACE_NOW.store(T0 + 290_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 2).is_none());
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(restart_clock, RESTART_NOW);

#[test]
fn restarts_offer_an_unacknowledged_rescue_again_identically_and_never_twice() {
    let directory = scratch("restart");
    let fake = FakeTransport::new();
    let first_ts = ts(T0 + 10_000);
    let second_ts = ts(T0 + 20_000);
    fake.push_history(listed(&first_ts, None, OWNER, "first", &["F0DDDDDDDD1"]));
    fake.push_history(listed(&second_ts, None, OWNER, "second", &[]));
    RESTART_NOW.store(T0 + 100_000, Ordering::SeqCst);
    fake.push_script(heartbeats(4));
    let resume = "slack1:seq:9";
    let mut backend = backend(&fake, restart_clock);

    // The ledger began at T0 (an earlier subscription); this one resumes at sequence 9.
    {
        let (mut ledger, _) = chat_subscription_slack::rescan::Ledger::load(
            directory.join(LEDGER_FILE),
            u128::from(T0) * 1_000,
        );
        ledger.mark_carried(&format!("{CHANNEL}/{}", ts(T0 + 1_000)));
        ledger.cursor = Some(resume.to_owned());
        ledger.save().unwrap();
    }
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, Some(resume), json!({})))
            .unwrap();
    let offered = next_batch_within(&mut subscription, 1).expect("first rescue");
    assert_eq!(
        offered.cursor().as_str(),
        format!("{resume};rescued:{CHANNEL}/{first_ts}")
    );
    // The bridge does not acknowledge (it rolled back): the plugin restarts.
    subscription.close().unwrap();

    fake.push_script(heartbeats(4));
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, Some(resume), json!({})))
            .unwrap();
    let again = next_batch_within(&mut subscription, 1).expect("offered again first");
    assert_eq!(again.cursor(), offered.cursor());
    assert_eq!(again.events(), offered.events());
    subscription.commit_durable(&again).unwrap();
    let second = next_batch_within(&mut subscription, 1).expect("second rescue");
    assert_eq!(
        second.cursor().as_str(),
        format!("{resume};rescued:{CHANNEL}/{second_ts}"),
        "a rescue cursor builds on the stream cursor, not on the previous rescue"
    );
    // The bridge commits it but the acknowledgement is lost with the plugin.
    subscription.close().unwrap();

    // Resumed from the second rescue's cursor: it was committed; nothing is offered again, and
    // the stream resumes at sequence 9.
    let mut script = heartbeats(2);
    script.push(stream_message(10, &first_ts, None, "late copy of first"));
    script.push(stream_message(11, &second_ts, None, "late copy of second"));
    fake.push_script(script);
    RESTART_NOW.store(T0 + 400_000, Ordering::SeqCst);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(&directory, Some(second.cursor().as_str()), json!({})),
    )
    .unwrap();
    assert_eq!(
        fake.state().opened.last().unwrap().catchup,
        chat_subscription_slack::transport::Catchup::Sequence(9)
    );
    for expected in ["slack1:seq:10", "slack1:seq:11"] {
        let skipped = next_batch_within(&mut subscription, 3).expect("late copies");
        assert_eq!(skipped.cursor().as_str(), expected);
        assert_eq!(skipped.events(), [CommittableEvent::Checkpoint]);
        subscription.commit_durable(&skipped).unwrap();
    }
    assert!(next_batch_within(&mut subscription, 2).is_none());
    subscription.close().unwrap();
    let saved = ledger(&directory);
    assert_eq!(saved["pending"], Value::Null);
    assert_eq!(saved["rescued"].as_array().unwrap().len(), 2);
    assert!(log(&directory).contains("committed the rescued message"));
    fs::remove_dir_all(&directory).unwrap();
}

clock!(backoff_clock, BACKOFF_NOW);

#[test]
fn failed_listings_back_off_and_an_unlisting_transport_stops_the_rescan() {
    let directory = scratch("backoff");
    let fake = FakeTransport::new();
    fake.push_script(heartbeats(12));
    fake.state()
        .scan_failures
        .push_back(TransportError::not_applied(
            "ratelimited",
            "slow down",
            true,
        ));
    fake.state()
        .scan_failures
        .push_back(TransportError::not_applied(
            "ratelimited",
            "slow down",
            true,
        ));
    let mut backend = backend(&fake, backoff_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    commit_next(&mut subscription);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(fake.state().scans.len(), 1);
    // One failure: the next pass waits two intervals (180 s), not one.
    BACKOFF_NOW.store(T0 + 100_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(fake.state().scans.len(), 1);
    BACKOFF_NOW.store(T0 + 181_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(fake.state().scans.len(), 2);
    // Two in a row: four intervals.
    BACKOFF_NOW.store(T0 + 181_000 + 359_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(fake.state().scans.len(), 2);
    BACKOFF_NOW.store(T0 + 181_000 + 361_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(fake.state().scans.len(), 3, "success resets the backoff");
    BACKOFF_NOW.store(T0 + 181_000 + 361_000 + 91_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(fake.state().scans.len(), 4);
    fake.state()
        .scan_failures
        .push_back(TransportError::not_applied(
            "scan_unsupported",
            "no listings",
            false,
        ));
    BACKOFF_NOW.store(T0 + 2_000_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    BACKOFF_NOW.store(T0 + 9_000_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 2).is_none());
    assert_eq!(
        fake.state().scans.len(),
        5,
        "an unlisting transport is not asked again"
    );
    subscription.close().unwrap();
    let log = log(&directory);
    assert!(log.contains("rescan: a listing failed after 0 succeeded (ratelimited: slow down); next pass in 180 s (failure 1 in a row)"));
    assert!(log.contains("rescan stopped"));
    fs::remove_dir_all(&directory).unwrap();
}

clock!(off_clock);

#[test]
fn the_rescan_is_off_when_disabled_or_without_a_diagnostics_log() {
    let directory = scratch("off");
    let fake = FakeTransport::new();
    fake.push_script(heartbeats(3));
    let mut backend = backend(&fake, off_clock);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(&directory, None, json!({"rescan_interval_seconds": 0})),
    )
    .unwrap();
    commit_next(&mut subscription);
    assert!(next_batch_within(&mut subscription, 3).is_none());
    subscription.close().unwrap();
    assert!(fake.state().scans.is_empty());
    // The ledger is kept even with the rescan off, so turning it on never re-offers a message.
    assert!(directory.join(LEDGER_FILE).exists());

    fake.push_script(heartbeats(3));
    let mut data = data(&directory, json!({}));
    data.remove("diagnostics_log");
    let without_log = SubscribeRequest::new(
        vec![ChannelId::new(CHANNEL).unwrap()],
        vec![SenderId::new(OWNER).unwrap()],
        None,
    )
    .unwrap()
    .with_backend_configuration(BackendConfiguration::new("agentctl.slack.v1", data).unwrap());
    let mut subscription = ChatSubscription::open(&mut backend, &without_log).unwrap();
    commit_next(&mut subscription);
    assert!(next_batch_within(&mut subscription, 3).is_none());
    subscription.close().unwrap();
    assert!(fake.state().scans.is_empty());

    for (extra, message) in [
        (
            json!({"rescan_interval_seconds": 10}),
            "rescan_interval_seconds",
        ),
        (
            json!({"rescan_interval_seconds": 3_601}),
            "rescan_interval_seconds",
        ),
        (
            json!({"rescan_window_seconds": 60}),
            "rescan_window_seconds",
        ),
    ] {
        let error = ChatSubscription::open(&mut backend, &request(&directory, None, extra))
            .err()
            .expect("invalid configuration");
        assert!(format!("{error:?}").contains(message), "{error:?}");
    }
    fs::remove_dir_all(&directory).unwrap();
}

clock!(host_clock, HOST_NOW);

fn host_state(root: &Path, directory: &Path) -> BridgeState {
    let configuration = serde_json::from_value(json!({
        "subscription_plugin": "slack",
        "channel_ids": [CHANNEL],
        "allowed_senders": [OWNER],
        "agent_name": "coordinator",
        "agent_label": "test coordinator",
        "ack_reaction": null,
        "receipt_reaction": null,
        "backend_configuration": {"schema": "agentctl.slack.v1", "data": data(directory, json!({}))},
    }))
    .expect("bridge configuration");
    BridgeState::initialize(root, configuration).expect("initialize bridge")
}

/// Consume until `limit` heartbeats in a row; returns the new request keys.
fn drain(subscription: &mut ChatSubscription, state: &BridgeState, limit: usize) -> Vec<String> {
    let mut keys = Vec::new();
    let mut quiet = 0;
    while quiet < limit {
        match consume_one(subscription, state).expect("bridge admits every batch") {
            ConsumedItem::Heartbeat => quiet += 1,
            ConsumedItem::Batch(admission) => {
                quiet = 0;
                keys.extend(admission.new_request_keys);
            }
            ConsumedItem::End => break,
        }
    }
    keys
}

#[test]
fn the_bridge_admits_each_message_once_across_stream_rescan_and_restart() {
    let directory = scratch("host");
    let root = directory.join("bridge");
    let state = host_state(&root, &directory);
    let fake = FakeTransport::new();
    let carried = ts(T0 + 5_000);
    let dropped = ts(T0 + 6_000);
    let mut script = vec![stream_message(20, &carried, None, "carried")];
    script.extend(heartbeats(6));
    fake.push_script(script);
    fake.push_history(listed(&carried, None, OWNER, "carried, edited since", &[]));
    fake.push_history(listed(&dropped, None, OWNER, "", &["F0EEEEEEEE1"]));
    let mut backend = backend(&fake, host_clock);

    let mut subscription =
        ChatSubscription::open(&mut backend, &state.subscribe_request().unwrap()).unwrap();
    let mut keys = drain(&mut subscription, &state, 2);
    assert_eq!(keys.len(), 1, "the carried message only");
    HOST_NOW.store(T0 + 100_000, Ordering::SeqCst);
    keys.extend(drain(&mut subscription, &state, 3));
    assert_eq!(keys.len(), 2, "plus the rescued file message");
    subscription.close().unwrap();

    // Restart: the stream replays nothing new, the rescan finds both again in history.
    let reopened = BridgeState::open(&root).unwrap();
    let mut script = heartbeats(2);
    script.push(stream_message(21, &dropped, None, "late copy"));
    script.extend(heartbeats(4));
    fake.push_script(script);
    HOST_NOW.store(T0 + 300_000, Ordering::SeqCst);
    let mut subscription =
        ChatSubscription::open(&mut backend, &reopened.subscribe_request().unwrap()).unwrap();
    assert!(drain(&mut subscription, &reopened, 4).is_empty());
    subscription.close().unwrap();

    let mut requests = reopened.request_keys().unwrap();
    requests.sort();
    keys.sort();
    assert_eq!(requests, keys);
    // The saved request records hold the delivered text.
    let texts: Vec<String> = keys
        .iter()
        .map(|key| fs::read_to_string(root.join("requests").join(format!("{key}.json"))).unwrap())
        .collect();
    assert!(texts
        .iter()
        .any(|text| text.contains("F0EEEEEEEE1") && text.contains("slack.file download")));
    assert!(texts.iter().all(|text| !text.contains("edited since")));
    fs::remove_dir_all(&directory).unwrap();
}

clock!(settle_clock, SETTLE_NOW);

#[test]
fn no_pass_runs_before_the_resumed_stream_has_caught_up() {
    // After a rolled-back stream batch the bridge needs that batch again first; a rescued batch
    // offered before the tail replays it would be refused.
    let directory = scratch("settle");
    let fake = FakeTransport::new();
    let resume = "slack1:seq:7";
    {
        let (mut ledger, _) = chat_subscription_slack::rescan::Ledger::load(
            directory.join(LEDGER_FILE),
            u128::from(T0) * 1_000,
        );
        ledger.cursor = Some(resume.to_owned());
        ledger.save().unwrap();
    }
    SETTLE_NOW.store(T0 + 100_000, Ordering::SeqCst);
    let replayed = ts(T0 + 5_000);
    let dropped = ts(T0 + 10_000);
    fake.push_history(listed(&dropped, None, OWNER, "", &["F0FFFFFFFF1"]));
    // The tail is slow to start: plain idle waits, then the replayed event, then heartbeats.
    let mut script = vec![FakeStep::Idle, FakeStep::Idle];
    script.push(stream_message(8, &replayed, None, "replayed"));
    script.extend(heartbeats(3));
    fake.push_script(script);
    let mut backend = backend(&fake, settle_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, Some(resume), json!({})))
            .unwrap();
    let first = next_batch_within(&mut subscription, 3).expect("the replayed stream batch");
    assert_eq!(first.cursor().as_str(), "slack1:seq:8");
    assert!(
        fake.state().scans.is_empty(),
        "no listing before the stream caught up"
    );
    subscription.commit_durable(&first).unwrap();
    let rescued = next_batch_within(&mut subscription, 2).expect("then the rescue");
    assert_eq!(
        rescued.cursor().as_str(),
        format!("slack1:seq:8;rescued:{CHANNEL}/{dropped}")
    );
    subscription.commit_durable(&rescued).unwrap();
    subscription.close().unwrap();
    assert_eq!(ledger(&directory)["cursor"], rescued.cursor().as_str());
    fs::remove_dir_all(&directory).unwrap();
}

clock!(later_clock, LATER_NOW);

#[test]
fn turning_the_rescan_on_later_never_offers_what_the_stream_delivered() {
    let directory = scratch("later");
    let fake = FakeTransport::new();
    let carried = ts(T0 + 5_000);
    let mut script = vec![stream_message(5, &carried, None, "original")];
    script.extend(heartbeats(2));
    fake.push_script(script);
    let mut backend = backend(&fake, later_clock);
    let off = json!({"rescan_interval_seconds": 0});
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, off)).unwrap();
    commit_next(&mut subscription);
    let delivered = commit_next(&mut subscription);
    assert!(next_batch_within(&mut subscription, 2).is_none());
    subscription.close().unwrap();
    assert!(fake.state().scans.is_empty());

    // On again, resumed from where the bridge is: the carried message, edited since, stays
    // delivered once; a dropped one is rescued.
    let dropped = ts(T0 + 7_000);
    fake.push_history(listed(&carried, None, OWNER, "edited", &[]));
    fake.push_history(listed(&dropped, None, OWNER, "dropped", &[]));
    fake.push_script(heartbeats(4));
    LATER_NOW.store(T0 + 100_000, Ordering::SeqCst);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(&directory, Some(delivered.cursor().as_str()), json!({})),
    )
    .unwrap();
    let rescued = next_batch_within(&mut subscription, 2).expect("the dropped message");
    assert!(single_message(&rescued).text().starts_with("dropped"));
    subscription.commit_durable(&rescued).unwrap();
    assert!(next_batch_within(&mut subscription, 2).is_none());
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(foreign_clock, FOREIGN_NOW);

#[test]
fn a_resume_the_ledger_did_not_see_rescues_nothing_older() {
    // Something without a ledger (an older plugin, or one with the rescan off and no log)
    // moved the bridge from sequence 3 to 9; what it delivered was never recorded carried.
    let directory = scratch("foreign");
    {
        let (mut ledger, _) = chat_subscription_slack::rescan::Ledger::load(
            directory.join(LEDGER_FILE),
            u128::from(T0) * 1_000,
        );
        ledger.cursor = Some("slack1:seq:3".to_owned());
        ledger.save().unwrap();
    }
    let fake = FakeTransport::new();
    fake.push_history(listed(
        &ts(T0 + 10_000),
        None,
        OWNER,
        "delivered unrecorded",
        &[],
    ));
    fake.push_script(heartbeats(3));
    FOREIGN_NOW.store(T0 + 100_000, Ordering::SeqCst);
    let mut backend = backend(&fake, foreign_clock);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(&directory, Some("slack1:seq:9"), json!({})),
    )
    .unwrap();
    assert!(next_batch_within(&mut subscription, 3).is_none());
    assert!(!fake.state().scans.is_empty());
    subscription.close().unwrap();
    assert!(log(&directory).contains("not from the ledger's last acknowledged cursor slack1:seq:3"));
    assert_eq!(
        ledger(&directory)["tracking_since_micros"],
        json!(u64::try_from(u128::from(T0 + 100_000) * 1_000).unwrap())
    );
    fs::remove_dir_all(&directory).unwrap();
}

clock!(active_clock, ACTIVE_NOW);

#[test]
fn an_active_thread_older_than_the_window_is_listed() {
    let directory = scratch("active");
    let fake = FakeTransport::new();
    // The root is two hours old, outside a ten-minute window, so history does not list it.
    let root = ts(T0 - 7_200_000);
    let carried_reply = ts(T0 + 5_000);
    let file_reply = ts(T0 + 20_000);
    fake.push_history(listed(&root, None, OWNER, "old root", &[]));
    fake.push_history(listed(
        &carried_reply,
        Some(&root),
        OWNER,
        "text reply",
        &[],
    ));
    fake.push_history(listed(
        &file_reply,
        Some(&root),
        OWNER,
        "",
        &["F0GGGGGGGG1"],
    ));
    let mut script = vec![stream_message(4, &carried_reply, Some(&root), "text reply")];
    script.extend(heartbeats(3));
    fake.push_script(script);
    let mut backend = backend(&fake, active_clock);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(&directory, None, json!({"rescan_window_seconds": 600})),
    )
    .unwrap();
    commit_next(&mut subscription);
    commit_next(&mut subscription);
    ACTIVE_NOW.store(T0 + 100_000, Ordering::SeqCst);
    let rescued = next_batch_within(&mut subscription, 2).expect("the file reply");
    let reply = single_message(&rescued);
    assert_eq!(
        reply.message_id().as_str(),
        format!("{CHANNEL}/messages/{file_reply}")
    );
    assert_eq!(
        reply.thread_id().as_str(),
        format!("{CHANNEL}/threads/{root}")
    );
    subscription.commit_durable(&rescued).unwrap();
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(ratelimit_clock, RATELIMIT_NOW);

#[test]
fn rate_limited_listings_retry_and_still_deliver_file_messages() {
    // Live regression 2026-10-28 (#615 chat-file-messages): Slack rate-limited
    // conversations.history (429, retry after 1-2 s) for hours. Every pass aborted on the
    // first failed listing and backed off exponentially to 30 min, so two owner messages
    // with screenshots were never rescued. A rate-limited listing must be retried after
    // its retry_after within the same pass.
    let directory = scratch("ratelimit");
    let fake = FakeTransport::new();
    fake.push_script(heartbeats(6));
    for _ in 0..2 {
        fake.state()
            .scan_failures
            .push_back(TransportError::not_applied(
                "slack_rate_limited",
                "Slack rate-limited request conversations.history. Retry after 0 seconds. \
                 {\"retry_after_seconds\":0}",
                true,
            ));
    }
    let files_only = ts(T0 + 10_000);
    let text_and_file = ts(T0 + 20_000);
    fake.push_history(listed(&files_only, None, OWNER, "", &["F0JJJJJJJJ1"]));
    fake.push_history(listed(
        &text_and_file,
        None,
        OWNER,
        "look at this",
        &["F0JJJJJJJJ2"],
    ));
    let mut backend = backend(&fake, ratelimit_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    commit_next(&mut subscription);
    RATELIMIT_NOW.store(T0 + 100_000, Ordering::SeqCst);
    let first = next_batch_within(&mut subscription, 2).expect("files-only rescue");
    let message = single_message(&first);
    assert_eq!(
        message.message_id().as_str(),
        format!("{CHANNEL}/messages/{files_only}")
    );
    assert!(message.text().contains("1 attached file: F0JJJJJJJJ1."));
    assert!(message.text().contains("--file-id=F0JJJJJJJJ1"));
    let payload = message.provider_payload().unwrap().data();
    assert_eq!(payload["file_ids"], json!(["F0JJJJJJJJ1"]));
    assert_eq!(payload["delivered_by"], "rescan");
    subscription.commit_durable(&first).unwrap();
    let second = next_batch_within(&mut subscription, 2).expect("text+file rescue");
    let message = single_message(&second);
    assert!(message.text().starts_with("look at this"));
    assert!(message.text().contains("1 attached file: F0JJJJJJJJ2."));
    subscription.commit_durable(&second).unwrap();
    assert!(
        fake.state().scans.len() >= 3,
        "two rate-limited attempts then a successful listing: {:?}",
        fake.state().scans
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(activity_clock, ACTIVITY_NOW);

#[test]
fn channel_activity_pulls_the_rescan_forward() {
    // #615: a file message beside ordinary traffic must arrive within seconds, not at
    // the next 90 s interval. A stream-carried message pulls the next pass to +20 s.
    let directory = scratch("activity");
    let fake = FakeTransport::new();
    let carried = ts(T0 + 5_000);
    let file_ts = ts(T0 + 10_000);
    let mut script = vec![stream_message(9, &carried, None, "hello")];
    script.extend(heartbeats(6));
    fake.push_script(script);
    fake.push_history(listed(&file_ts, None, OWNER, "", &["F0KKKKKKKK1"]));
    let mut backend = backend(&fake, activity_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    commit_next(&mut subscription);
    let plain = commit_next(&mut subscription);
    assert_eq!(single_message(&plain).text(), "hello");
    ACTIVITY_NOW.store(T0 + 30_000, Ordering::SeqCst);
    let rescued = next_batch_within(&mut subscription, 2).expect("activity-pulled rescue");
    assert_eq!(
        single_message(&rescued).message_id().as_str(),
        format!("{CHANNEL}/messages/{file_ts}")
    );
    subscription.commit_durable(&rescued).unwrap();
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(overflow_clock, OVERFLOW_NOW);

#[test]
fn replies_left_out_of_a_full_queue_are_listed_again() {
    let directory = scratch("overflow");
    let fake = FakeTransport::new();
    let root = ts(T0 + 1_000);
    fake.push_history(listed(&root, None, OWNER, "root", &[]));
    let replies: Vec<String> = (0..34).map(|n| ts(T0 + 2_000 + n * 100)).collect();
    for reply in &replies {
        fake.push_history(listed(reply, Some(&root), OWNER, "", &["F0HHHHHHHH1"]));
    }
    let mut script = vec![stream_message(2, &root, None, "root")];
    script.extend(heartbeats(60));
    fake.push_script(script);
    let mut backend = backend(&fake, overflow_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    commit_next(&mut subscription);
    commit_next(&mut subscription);
    OVERFLOW_NOW.store(T0 + 100_000, Ordering::SeqCst);
    let mut delivered = Vec::new();
    for _ in 0..32 {
        let batch = next_batch_within(&mut subscription, 2).expect("a queued reply");
        delivered.push(single_message(&batch).message_id().as_str().to_owned());
        subscription.commit_durable(&batch).unwrap();
    }
    assert!(next_batch_within(&mut subscription, 1).is_none());
    OVERFLOW_NOW.store(T0 + 200_000, Ordering::SeqCst);
    while let Some(batch) = next_batch_within(&mut subscription, 2) {
        delivered.push(single_message(&batch).message_id().as_str().to_owned());
        subscription.commit_durable(&batch).unwrap();
    }
    subscription.close().unwrap();
    let expected: Vec<String> = replies
        .iter()
        .map(|reply| format!("{CHANNEL}/messages/{reply}"))
        .collect();
    assert_eq!(delivered, expected);
    fs::remove_dir_all(&directory).unwrap();
}
