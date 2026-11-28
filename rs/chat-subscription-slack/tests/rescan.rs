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
        // The host-wide listing budget is off: tests must not share the user's real one.
        ("rescan_budget_key".to_owned(), json!("off")),
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

clock!(yield_clock, YIELD_NOW);

// #679 muse-bridge-study: a pass ran in the driver loop, so a live message that arrived while it
// listed threads or waited out a rate limit waited for the whole pass (up to a minute in a
// channel with many threads). After its first history listing a pass yields to a waiting event.
#[test]
fn a_rescan_pass_yields_to_a_live_message_waiting_on_the_stream() {
    let directory = scratch("yield");
    let fake = FakeTransport::new();
    let root_ts = ts(T0 + 1_000);
    let reply_ts = ts(T0 + 5_000);
    let live_ts = ts(T0 + 6_000);
    let mut script = heartbeats(1);
    script.push(stream_message(43, &live_ts, None, "live"));
    fake.push_script(script);
    // The bridge's own post and reply: nothing to rescue, but a thread a pass would list.
    fake.push_history(listed(&root_ts, None, BOT, "root", &[]));
    fake.push_history(listed(&reply_ts, Some(&root_ts), BOT, "reply", &[]));
    let mut backend = backend(&fake, yield_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    commit_next(&mut subscription);
    // Past the reply's grace period, so the thread is due for a listing.
    YIELD_NOW.store(T0 + 30_000, Ordering::SeqCst);

    // The heartbeat starts a pass; its history listing finds a thread to list, but the live
    // message is already waiting, so the pass stops before listing the thread.
    let live = next_batch_within(&mut subscription, 2).expect("the live message");
    assert_eq!(single_message(&live).text(), "live");
    assert_eq!(fake.state().scans.len(), 1, "only the history listing ran");
    assert!(log(&directory).contains("rescan: yielded to a live event after 1 listings"));
    subscription.commit_durable(&live).unwrap();

    // The yield adds no pass of its own; the live message's channel activity pulls the next
    // one forward as usual, and that pass lists the thread.
    YIELD_NOW.store(T0 + 60_000, Ordering::SeqCst);
    let _ = next_batch_within(&mut subscription, 2);
    assert!(
        fake.state().scans.len() >= 3,
        "{:?}",
        fake.state().scans.len()
    );
    subscription.close().unwrap();
}

fn thread_scans(fake: &FakeTransport) -> usize {
    fake.state()
        .scans
        .iter()
        .filter(|scan| scan.1.is_some())
        .count()
}

clock!(ripe_clock, RIPE_NOW);

// #690 muse-bridge-rescan-intake: a thread whose newest reply is still in its grace period is
// not listed (nothing in it could be rescued yet), a later pass lists it once, and a pass that
// finds its history row unchanged does not list it again.
#[test]
fn a_thread_is_listed_once_its_newest_reply_is_ripe_and_not_again_while_unchanged() {
    let directory = scratch("ripe");
    let fake = FakeTransport::new();
    let root_ts = ts(T0 + 1_000);
    fake.push_history(listed(&root_ts, None, BOT, "root", &[]));
    fake.push_history(listed(&ts(T0 + 5_000), Some(&root_ts), BOT, "reply", &[]));
    fake.push_script(heartbeats(8));
    let mut backend = backend(&fake, ripe_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    commit_next(&mut subscription);

    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(fake.state().scans.len(), 1, "only the history listing");
    assert_eq!(thread_scans(&fake), 0);
    assert!(log(&directory).contains("1 thread postponed"));

    RIPE_NOW.store(T0 + 100_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(thread_scans(&fake), 1, "listed once it was ripe");

    RIPE_NOW.store(T0 + 200_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(thread_scans(&fake), 1, "not listed again while unchanged");
    assert!(fake.state().scans.len() >= 3);
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(carriedreply_clock, CARRIEDREPLY_NOW);

// #690 muse-bridge-rescan-intake: once a thread was listed, new replies the stream carried
// leave nothing to rescue in it, so its changed history row does not bring another listing.
#[test]
fn a_thread_whose_new_replies_the_stream_carried_is_not_listed_again() {
    let directory = scratch("carriedreply");
    let fake = FakeTransport::new();
    let root_ts = ts(T0 + 1_000);
    let first = ts(T0 + 5_000);
    let second = ts(T0 + 150_000);
    fake.push_history(listed(&root_ts, None, BOT, "root", &[]));
    fake.push_history(listed(&first, Some(&root_ts), BOT, "first", &[]));
    let mut script = heartbeats(3);
    script.push(stream_message(7, &second, Some(&root_ts), "second"));
    script.extend(heartbeats(6));
    fake.push_script(script);
    let mut backend = backend(&fake, carriedreply_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    commit_next(&mut subscription);

    // The first pass postpones the thread; one later lists it.
    assert!(next_batch_within(&mut subscription, 1).is_none());
    CARRIEDREPLY_NOW.store(T0 + 100_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(thread_scans(&fake), 1);

    // The owner's reply arrives on the stream; history now shows it as the newest reply.
    CARRIEDREPLY_NOW.store(T0 + 151_000, Ordering::SeqCst);
    fake.push_history(listed(&second, Some(&root_ts), OWNER, "second", &[]));
    let reply = next_batch_within(&mut subscription, 3).expect("the carried reply");
    assert_eq!(single_message(&reply).text(), "second");
    subscription.commit_durable(&reply).unwrap();

    CARRIEDREPLY_NOW.store(T0 + 400_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 2).is_none());
    assert_eq!(
        thread_scans(&fake),
        1,
        "the carried reply needed no listing"
    );
    assert!(log(&directory).contains("1 not listed (new replies all carried)"));
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(hostbudget_clock, HOSTBUDGET_NOW);

// #690 muse-bridge-rescan-intake: the host-wide budget, shared by every bridge listing as one
// identity, stops a pass that would exceed it; the next pass waits for the next minute.
#[test]
fn a_spent_host_budget_stops_the_pass_before_its_thread_listing() {
    let directory = scratch("hostbudget");
    let fake = FakeTransport::new();
    let root_ts = ts(T0 + 1_000);
    fake.push_history(listed(&root_ts, None, BOT, "root", &[]));
    fake.push_history(listed(&ts(T0 + 5_000), Some(&root_ts), BOT, "reply", &[]));
    fake.push_script(heartbeats(4));
    let mut backend = backend(&fake, hostbudget_clock);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(
            &directory,
            None,
            json!({
                "rescan_budget_key": directory.join("budget"),
                "rescan_calls_per_minute": 1
            }),
        ),
    )
    .unwrap();
    commit_next(&mut subscription);
    // Past the reply's grace period, so the thread is due for a listing.
    HOSTBUDGET_NOW.store(T0 + 30_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(
        fake.state().scans.len(),
        1,
        "the thread listing was refused"
    );
    let text = log(&directory);
    assert!(
        text.contains("host budget 1 of 1 calls this minute"),
        "{text}"
    );
    assert!(text.contains("is spent for this minute"), "{text}");
    assert!(directory.join("budget/budget.json").is_file());
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(busy_clock, BUSY_NOW);

// #690 muse-bridge-rescan-intake (review): a thread whose newest reply is always young, such as
// one the agent keeps posting progress in, is postponed one pass at most, then listed anyway.
#[test]
fn a_busy_thread_is_postponed_one_pass_at_most() {
    let directory = scratch("busythread");
    let fake = FakeTransport::new();
    let root_ts = ts(T0 + 1_000);
    fake.push_history(listed(&root_ts, None, BOT, "root", &[]));
    fake.push_history(listed(&ts(T0 + 5_000), Some(&root_ts), BOT, "first", &[]));
    fake.push_script(heartbeats(8));
    let mut backend = backend(&fake, busy_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, json!({}))).unwrap();
    commit_next(&mut subscription);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(
        thread_scans(&fake),
        0,
        "postponed: its newest reply is young"
    );

    // A new reply keeps the newest one young at the next pass; it is listed anyway.
    fake.push_history(listed(
        &ts(T0 + 95_000),
        Some(&root_ts),
        BOT,
        "progress",
        &[],
    ));
    BUSY_NOW.store(T0 + 100_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 1).is_none());
    assert_eq!(thread_scans(&fake), 1, "listed at the second pass");
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(budgetretry_clock, BUDGETRETRY_NOW);

// #690 muse-bridge-rescan-intake (review): with a host budget, the back-off a rate-limited
// listing records holds the other bridges, but not this bridge's own retry after the wait Slack
// named, so #615's in-pass retry still rescues file messages.
#[test]
fn a_rate_limited_listing_is_retried_in_the_pass_with_a_host_budget() {
    let directory = scratch("budgetretry");
    let fake = FakeTransport::new();
    fake.push_script(heartbeats(6));
    fake.state()
        .scan_failures
        .push_back(TransportError::not_applied(
            "slack_rate_limited",
            "Slack rate-limited request conversations.history. Retry after 0 seconds. \
             {\"retry_after_seconds\":0}",
            true,
        ));
    let file_ts = ts(T0 + 10_000);
    fake.push_history(listed(&file_ts, None, OWNER, "", &["F0KKKKKKKK1"]));
    let mut backend = backend(&fake, budgetretry_clock);
    let budget = directory.join("budget");
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(&directory, None, json!({"rescan_budget_key": budget})),
    )
    .unwrap();
    commit_next(&mut subscription);
    BUDGETRETRY_NOW.store(T0 + 100_000, Ordering::SeqCst);
    let rescued = next_batch_within(&mut subscription, 2).expect("rescued after the retry");
    assert!(single_message(&rescued).text().contains("F0KKKKKKKK1"));
    subscription.commit_durable(&rescued).unwrap();
    // The back-off it recorded holds any other bridge of this budget.
    let state: Value =
        serde_json::from_slice(&fs::read(budget.join("budget.json")).unwrap()).unwrap();
    assert!(state["blocked_until_millis"].as_u64().unwrap() >= T0 + 100_000 + 30_000);
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

// --- #769 relay-rescan-settle-latency: relay-only fast poll ---

clock!(relay_clock, RELAY_NOW);
clock!(relay_budget_clock, RELAY_BUDGET_NOW);

const RELAY_PEER: &str = "U0RELAYBOT1";

fn relay_extra() -> Value {
    json!({
        "relay_channels": [{"channel": CHANNEL, "peer_senders": [RELAY_PEER]}],
        "relay_poll_seconds": 5,
    })
}

/// Like `next_batch_within` with more turns: under the gate's load an
/// idle turn waits out a real heartbeat, so a poll can need several.
fn next_batch_long(subscription: &mut ChatSubscription) -> Option<DeliveryBatch> {
    next_batch_within(subscription, 12)
}

fn relay_request(directory: &Path, extra: Value) -> SubscribeRequest {
    request(directory, None, extra)
        .with_relay_senders(vec![SenderId::new(RELAY_PEER).unwrap()])
        .unwrap()
        .with_relay_channels(vec![ChannelId::new(CHANNEL).unwrap()])
        .unwrap()
}

#[test]
fn relay_post_is_admitted_within_one_poll_with_no_settle_and_no_grace() {
    let directory = scratch("relay-poll");
    let fake = FakeTransport::new();
    // No heartbeats, no events: the stream never settles in this test, and
    // the fake clock stays minutes short of the 10-minute settle fallback.
    fake.push_script(vec![FakeStep::Idle; 8]);
    let root = ts(T0 + 10_000);
    let reply = ts(T0 + 11_000);
    let mut envelope = listed(&root, None, RELAY_PEER, "relay envelope", &[]);
    envelope.subtype = Some("bot_message".to_owned());
    fake.push_history(envelope);
    let mut receipt = listed(&reply, Some(&root), RELAY_PEER, "receipt: delivered", &[]);
    receipt.subtype = Some("bot_message".to_owned());
    fake.push_history(receipt);
    let mut backend = backend(&fake, relay_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, relay_extra())).unwrap();
    commit_next(&mut subscription);
    // Two seconds after the post: inside the general rescan's 15 s grace,
    // and with no settle, only the relay poll can admit it.
    RELAY_NOW.store(T0 + 12_000, Ordering::SeqCst);
    let batch = next_batch_long(&mut subscription).expect("relay batch within one poll");
    assert_eq!(
        single_message(&batch).message_id().as_str(),
        format!("{CHANNEL}/messages/{root}")
    );
    subscription.commit_durable(&batch).unwrap();
    // The receipt reply in its thread is listed and admitted by the same poll.
    let batch = next_batch_long(&mut subscription).expect("receipt batch");
    let message = single_message(&batch);
    assert!(message.is_thread_reply());
    assert_eq!(
        message.message_id().as_str(),
        format!("{CHANNEL}/messages/{reply}")
    );
    subscription.commit_durable(&batch).unwrap();
    assert!(
        fake.state()
            .scans
            .iter()
            .any(|(_, thread, _, _)| thread.as_deref() == Some(root.as_str())),
        "thread listed: {:?}",
        fake.state().scans
    );
    // Dedup against the general rescan: after the settle fallback the
    // general pass lists the same history and delivers nothing again.
    RELAY_NOW.store(T0 + 700_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 4).is_none());
    let saved = ledger(&directory);
    assert_eq!(saved["rescued"].as_array().unwrap().len(), 2, "{saved}");
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn relay_poll_budget_refusal_backs_off_without_listing() {
    let directory = scratch("relay-budget");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 10]);
    let root = ts(T0 + 10_000);
    let mut envelope = listed(&root, None, RELAY_PEER, "relay envelope", &[]);
    envelope.subtype = Some("bot_message".to_owned());
    fake.push_history(envelope);
    // Another bridge's relay poll was told to back off: the relay budget
    // holds (#1892: the relay poll's own budget, never the owner rescan's).
    let budget =
        chat_subscription_slack::budget::CallBudget::at(&directory.join("budget-relay.json"), 20);
    budget.block(T0 + 60_000);
    let mut extra = relay_extra();
    extra["rescan_budget_key"] = json!(directory.to_str().unwrap());
    let mut backend = backend(&fake, relay_budget_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, extra)).unwrap();
    commit_next(&mut subscription);
    RELAY_BUDGET_NOW.store(T0 + 12_000, Ordering::SeqCst);
    assert!(
        next_batch_within(&mut subscription, 3).is_none(),
        "budget holds the poll back"
    );
    assert!(
        fake.state().scans.is_empty(),
        "no listing was made: {:?}",
        fake.state().scans
    );
    assert!(
        log(&directory).contains("relay poll"),
        "{}",
        log(&directory)
    );
    // After the back-off the poll lists and delivers.
    RELAY_BUDGET_NOW.store(T0 + 125_000, Ordering::SeqCst);
    let batch = next_batch_long(&mut subscription).expect("relay batch after back-off");
    assert_eq!(
        single_message(&batch).message_id().as_str(),
        format!("{CHANNEL}/messages/{root}")
    );
    subscription.commit_durable(&batch).unwrap();
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(relay_human_clock, RELAY_HUMAN_NOW);
clock!(relay_headroom_clock, RELAY_HEADROOM_NOW);

#[test]
fn relay_poll_leaves_human_posts_to_the_stream_and_general_rescan() {
    // #769 review fix 1: only relay peers' posts (and their thread
    // replies) are polled; a human post in the relay channel is not.
    let directory = scratch("relay-human");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 10]);
    let peer_ts = ts(T0 + 10_000);
    let mut envelope = listed(&peer_ts, None, RELAY_PEER, "relay envelope", &[]);
    envelope.subtype = Some("bot_message".to_owned());
    fake.push_history(envelope);
    let human = listed(
        &ts(T0 + 10_500),
        None,
        OWNER,
        "human note in relay channel",
        &[],
    );
    fake.push_history(human);
    let mut backend = backend(&fake, relay_human_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, relay_extra())).unwrap();
    commit_next(&mut subscription);
    RELAY_HUMAN_NOW.store(T0 + 12_000, Ordering::SeqCst);
    let batch = next_batch_long(&mut subscription).expect("peer batch");
    assert_eq!(
        single_message(&batch).message_id().as_str(),
        format!("{CHANNEL}/messages/{peer_ts}")
    );
    subscription.commit_durable(&batch).unwrap();
    assert!(
        next_batch_within(&mut subscription, 3).is_none(),
        "human post is not admitted by the poll"
    );
    let saved = ledger(&directory);
    assert_eq!(saved["rescued"].as_array().unwrap().len(), 1, "{saved}");
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn relay_poll_stops_at_half_the_shared_budget() {
    // #769 review fix 2, on the relay poll's own budget (#1892): once half
    // its per-minute budget is used, the poll makes no listing.
    let directory = scratch("relay-headroom");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 10]);
    let root = ts(T0 + 10_000);
    let mut envelope = listed(&root, None, RELAY_PEER, "relay envelope", &[]);
    envelope.subtype = Some("bot_message".to_owned());
    fake.push_history(envelope);
    // The relay allowance is half the configured 4, so 2; spend half of it.
    let budget =
        chat_subscription_slack::budget::CallBudget::at(&directory.join("budget-relay.json"), 2);
    budget.take(T0).expect("pre-spend half the relay budget");
    let mut extra = relay_extra();
    extra["rescan_budget_key"] = json!(directory.to_str().unwrap());
    extra["rescan_calls_per_minute"] = json!(4);
    let mut backend = backend(&fake, relay_headroom_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, extra)).unwrap();
    commit_next(&mut subscription);
    RELAY_HEADROOM_NOW.store(T0 + 12_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut subscription, 3).is_none());
    assert!(
        fake.state().scans.is_empty(),
        "no listing at headroom: {:?}",
        fake.state().scans
    );
    assert!(log(&directory).contains("headroom"), "{}", log(&directory));
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(relay_floor_clock, RELAY_FLOOR_NOW);
clock!(relay_cap_clock, RELAY_CAP_NOW);

#[test]
fn relay_poll_faster_than_the_floor_is_clamped_and_logged() {
    // #1682 slack-rate-limit-budget: relay_extra asks for 5 s; the effective
    // poll is floored at 30 s, and the startup log says so.
    let directory = scratch("relay-floor");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 10]);
    let mut backend = backend(&fake, relay_floor_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, relay_extra())).unwrap();
    commit_next(&mut subscription);
    let logged = log(&directory);
    assert!(
        logged.contains("relay poll: every 30 s") && logged.contains("floored"),
        "{logged}"
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn relay_poll_lists_at_most_three_threads_and_keeps_the_rest_due() {
    // #1682 slack-rate-limit-budget: one poll lists at most three due
    // threads; the rest stay due and are listed by the next poll.
    let directory = scratch("relay-cap");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 14]);
    for i in 0..5_u64 {
        let root = ts(T0 + 10_000 + i * 2_000);
        let reply = ts(T0 + 11_000 + i * 2_000);
        fake.push_history(bot_message(&root, None, RELAY_PEER, "relay envelope"));
        fake.push_history(bot_message(&reply, Some(&root), RELAY_PEER, "first reply"));
    }
    let mut backend = backend(&fake, relay_cap_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, relay_extra())).unwrap();
    commit_next(&mut subscription);
    RELAY_CAP_NOW.store(T0 + 12_000, Ordering::SeqCst);
    drain_batches(&mut subscription);
    let first_threads: std::collections::BTreeSet<String> = fake
        .state()
        .scans
        .iter()
        .filter_map(|scan| scan.1.clone())
        .collect();
    assert_eq!(
        first_threads.len(),
        3,
        "first poll thread listings: {:?}",
        fake.state().scans
    );
    RELAY_CAP_NOW.store(T0 + 60_000, Ordering::SeqCst);
    drain_batches(&mut subscription);
    let all_threads: std::collections::BTreeSet<String> = fake
        .state()
        .scans
        .iter()
        .filter_map(|scan| scan.1.clone())
        .collect();
    assert_eq!(
        all_threads.len(),
        5,
        "every due thread listed across two polls: {:?}",
        fake.state().scans
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

// --- #805 relay-poll-receipts-during-hold ---

clock!(relay_hold_clock, RELAY_HOLD_NOW);
clock!(relay_hold_replies_clock, RELAY_HOLD_REPLIES_NOW);
clock!(relay_hold_bound_clock, RELAY_HOLD_BOUND_NOW);
clock!(relay_log_clock, RELAY_LOG_NOW);

fn bot_message(ts: &str, thread_ts: Option<&str>, user: &str, text: &str) -> SlackMessage {
    let mut message = listed(ts, thread_ts, user, text, &[]);
    message.subtype = Some("bot_message".to_owned());
    message
}

fn drain_batches(subscription: &mut ChatSubscription) {
    while let Some(batch) = next_batch_within(subscription, 3) {
        subscription.commit_durable(&batch).unwrap();
    }
}

#[test]
fn relay_poll_during_budget_hold_reads_receipt_on_tracked_thread() {
    // #805: with the relay budget holding after a rate limit, the relay
    // poll still lists the threads of tracked relay posts, so a receipt
    // posted during the hold is read within one poll interval.
    let directory = scratch("relay-hold");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 14]);
    let root = ts(T0 + 10_000);
    let reply = ts(T0 + 11_000);
    fake.push_history(bot_message(&root, None, RELAY_PEER, "relay envelope"));
    fake.push_history(bot_message(&reply, Some(&root), RELAY_PEER, "first reply"));
    let budget =
        chat_subscription_slack::budget::CallBudget::at(&directory.join("budget-relay.json"), 20);
    let mut extra = relay_extra();
    extra["rescan_budget_key"] = json!(directory.to_str().unwrap());
    let mut backend = backend(&fake, relay_hold_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, extra)).unwrap();
    commit_next(&mut subscription);
    RELAY_HOLD_NOW.store(T0 + 12_000, Ordering::SeqCst);
    drain_batches(&mut subscription);
    let tracked = fake.state().scans.len();
    assert!(
        fake.state().scans[..tracked]
            .iter()
            .any(|(_, thread, _, _)| thread.as_deref() == Some(root.as_str())),
        "thread tracked before the hold: {:?}",
        fake.state().scans
    );
    // Another bridge's relay poll was rate-limited listing history; the
    // relay budget now holds, caused by conversations.history (#1892).
    budget.block_for(T0 + 3_600_000, "conversations.history");
    let receipt = ts(T0 + 13_000);
    fake.push_history(bot_message(
        &receipt,
        Some(&root),
        RELAY_PEER,
        "receipt: delivered",
    ));
    // Past the floored 30 s poll interval (#1682), still inside the hold.
    RELAY_HOLD_NOW.store(T0 + 48_000, Ordering::SeqCst);
    let batch = next_batch_long(&mut subscription).expect("receipt read during the hold");
    let message = single_message(&batch);
    assert!(message.is_thread_reply());
    assert_eq!(
        message.message_id().as_str(),
        format!("{CHANNEL}/messages/{receipt}")
    );
    subscription.commit_durable(&batch).unwrap();
    let hold_scans = &fake.state().scans[tracked..];
    assert!(
        !hold_scans.is_empty() && hold_scans.len() <= 3,
        "bounded thread listings during the hold: {hold_scans:?}"
    );
    assert!(
        hold_scans
            .iter()
            .all(|(_, thread, _, _)| thread.as_deref() == Some(root.as_str())),
        "only thread listings during the hold, no history: {hold_scans:?}"
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn relay_poll_during_budget_hold_lists_at_most_three_threads() {
    // #805 bound: however many relay threads are tracked, one poll
    // during a hold makes at most 3 thread listings.
    let directory = scratch("relay-hold-bound");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 14]);
    let mut roots = Vec::new();
    for i in 0..4_u64 {
        let root = ts(T0 + 10_000 + i * 2_000);
        let reply = ts(T0 + 11_000 + i * 2_000);
        fake.push_history(bot_message(&root, None, RELAY_PEER, "relay envelope"));
        fake.push_history(bot_message(&reply, Some(&root), RELAY_PEER, "first reply"));
        roots.push(root);
    }
    let budget =
        chat_subscription_slack::budget::CallBudget::at(&directory.join("budget-relay.json"), 20);
    let mut extra = relay_extra();
    extra["rescan_budget_key"] = json!(directory.to_str().unwrap());
    // A wide configured allowance (relay gets half, 20) so the bound of
    // three held listings is what stops the poll, not the budget (#1892).
    extra["rescan_calls_per_minute"] = json!(40);
    let mut backend = backend(&fake, relay_hold_bound_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, extra)).unwrap();
    commit_next(&mut subscription);
    RELAY_HOLD_BOUND_NOW.store(T0 + 12_000, Ordering::SeqCst);
    drain_batches(&mut subscription);
    let tracked = fake.state().scans.len();
    budget.block_for(T0 + 3_600_000, "conversations.history");
    for (i, root) in roots.iter().enumerate() {
        let receipt = ts(T0 + 30_000 + i as u64 * 1_000);
        fake.push_history(bot_message(
            &receipt,
            Some(root),
            RELAY_PEER,
            "receipt: delivered",
        ));
    }
    RELAY_HOLD_BOUND_NOW.store(T0 + 48_000, Ordering::SeqCst);
    drain_batches(&mut subscription);
    let hold_scans = &fake.state().scans[tracked..];
    let thread_listings = hold_scans.iter().filter(|scan| scan.1.is_some()).count();
    assert_eq!(
        thread_listings, 3,
        "at most 3 thread listings in one held poll: {hold_scans:?}"
    );
    assert!(
        hold_scans.iter().all(|scan| scan.1.is_some()),
        "no history listings during the hold: {hold_scans:?}"
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn relay_poll_during_replies_caused_hold_lists_no_threads() {
    // #805 guard: a hold caused by conversations.replies itself must
    // not be spent listing threads; that method is the limited one.
    let directory = scratch("relay-hold-replies");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 14]);
    let root = ts(T0 + 10_000);
    let reply = ts(T0 + 11_000);
    fake.push_history(bot_message(&root, None, RELAY_PEER, "relay envelope"));
    fake.push_history(bot_message(&reply, Some(&root), RELAY_PEER, "first reply"));
    let budget =
        chat_subscription_slack::budget::CallBudget::at(&directory.join("budget-relay.json"), 20);
    let mut extra = relay_extra();
    extra["rescan_budget_key"] = json!(directory.to_str().unwrap());
    let mut backend = backend(&fake, relay_hold_replies_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, extra)).unwrap();
    commit_next(&mut subscription);
    RELAY_HOLD_REPLIES_NOW.store(T0 + 12_000, Ordering::SeqCst);
    drain_batches(&mut subscription);
    let tracked = fake.state().scans.len();
    budget.block_for(T0 + 3_600_000, "conversations.replies");
    let receipt = ts(T0 + 13_000);
    fake.push_history(bot_message(
        &receipt,
        Some(&root),
        RELAY_PEER,
        "receipt: delivered",
    ));
    RELAY_HOLD_REPLIES_NOW.store(T0 + 48_000, Ordering::SeqCst);
    assert!(
        next_batch_within(&mut subscription, 3).is_none(),
        "no receipt is read during a replies-caused hold"
    );
    assert_eq!(
        fake.state().scans.len(),
        tracked,
        "no listing at all during a replies-caused hold: {:?}",
        fake.state().scans
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(relay_hold_fresh_clock, RELAY_HOLD_FRESH_NOW);
clock!(relay_hold_unknown_clock, RELAY_HOLD_UNKNOWN_NOW);
clock!(relay_hold_limited_clock, RELAY_HOLD_LIMITED_NOW);

fn tracked_subscription(
    name: &str,
    clock: fn() -> u64,
    now: &AtomicU64,
    roots_and_replies: &[(&str, &str)],
) -> (ChatSubscription, FakeTransport, PathBuf, PathBuf) {
    let directory = scratch(name);
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 16]);
    for (root, reply) in roots_and_replies {
        fake.push_history(bot_message(root, None, RELAY_PEER, "relay envelope"));
        fake.push_history(bot_message(reply, Some(root), RELAY_PEER, "first reply"));
    }
    let mut extra = relay_extra();
    extra["rescan_budget_key"] = json!(directory.to_str().unwrap());
    let mut backend = backend(&fake, clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, extra)).unwrap();
    commit_next(&mut subscription);
    now.store(T0 + 12_000, Ordering::SeqCst);
    drain_batches(&mut subscription);
    let budget_path = directory.join("budget-relay.json");
    (subscription, fake, directory, budget_path)
}

#[test]
fn relay_poll_during_hold_reads_receipt_on_fresh_post_with_no_earlier_reply() {
    // #805 review fix 1: a relay post whose thread was never listed
    // (its first reply IS the receipt) is still a held-poll candidate,
    // because the poll tracks the root when it first sees it.
    let directory = scratch("relay-hold-fresh");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 14]);
    let root = ts(T0 + 10_000);
    fake.push_history(bot_message(&root, None, RELAY_PEER, "relay envelope"));
    let budget =
        chat_subscription_slack::budget::CallBudget::at(&directory.join("budget-relay.json"), 20);
    let mut extra = relay_extra();
    extra["rescan_budget_key"] = json!(directory.to_str().unwrap());
    let mut backend = backend(&fake, relay_hold_fresh_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, extra)).unwrap();
    commit_next(&mut subscription);
    RELAY_HOLD_FRESH_NOW.store(T0 + 12_000, Ordering::SeqCst);
    drain_batches(&mut subscription);
    let tracked = fake.state().scans.len();
    assert!(
        fake.state().scans[..tracked]
            .iter()
            .all(|scan| scan.1.is_none()),
        "no thread was listed before the hold: {:?}",
        fake.state().scans
    );
    budget.block_for(T0 + 3_600_000, "conversations.history");
    let receipt = ts(T0 + 13_000);
    fake.push_history(bot_message(
        &receipt,
        Some(&root),
        RELAY_PEER,
        "receipt: delivered",
    ));
    RELAY_HOLD_FRESH_NOW.store(T0 + 48_000, Ordering::SeqCst);
    let batch = next_batch_long(&mut subscription).expect("receipt on a fresh post read in hold");
    assert_eq!(
        single_message(&batch).message_id().as_str(),
        format!("{CHANNEL}/messages/{receipt}")
    );
    subscription.commit_durable(&batch).unwrap();
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn relay_poll_during_unknown_cause_hold_lists_no_threads() {
    // #805 review fix 2: a hold with no recorded method (written by an
    // older bridge, or by one whose own writes dropped the field) is
    // unsafe: list nothing.
    let root = ts(T0 + 10_000);
    let reply = ts(T0 + 11_000);
    let (mut subscription, fake, directory, budget_path) = tracked_subscription(
        "relay-hold-unknown",
        relay_hold_unknown_clock,
        &RELAY_HOLD_UNKNOWN_NOW,
        &[(&root, &reply)],
    );
    let tracked = fake.state().scans.len();
    let budget = chat_subscription_slack::budget::CallBudget::at(&budget_path, 20);
    budget.block(T0 + 3_600_000);
    let receipt = ts(T0 + 13_000);
    fake.push_history(bot_message(
        &receipt,
        Some(&root),
        RELAY_PEER,
        "receipt: delivered",
    ));
    RELAY_HOLD_UNKNOWN_NOW.store(T0 + 48_000, Ordering::SeqCst);
    assert!(
        next_batch_within(&mut subscription, 3).is_none(),
        "no receipt is read during an unknown-cause hold"
    );
    assert_eq!(
        fake.state().scans.len(),
        tracked,
        "no listing during an unknown-cause hold: {:?}",
        fake.state().scans
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn relay_poll_stops_held_listings_when_one_is_rate_limited() {
    // #805 review fix 3: a rate limit during the held listings stops
    // them; the poll must not go on listing against the limited method.
    let root_a = ts(T0 + 10_000);
    let reply_a = ts(T0 + 11_000);
    let root_b = ts(T0 + 20_000);
    let reply_b = ts(T0 + 21_000);
    let (mut subscription, fake, directory, budget_path) = tracked_subscription(
        "relay-hold-limited",
        relay_hold_limited_clock,
        &RELAY_HOLD_LIMITED_NOW,
        &[(&root_a, &reply_a), (&root_b, &reply_b)],
    );
    let tracked = fake.state().scans.len();
    let budget = chat_subscription_slack::budget::CallBudget::at(&budget_path, 20);
    budget.block_for(T0 + 3_600_000, "conversations.history");
    for (i, root) in [&root_a, &root_b].iter().enumerate() {
        let receipt = ts(T0 + 30_000 + i as u64 * 1_000);
        fake.push_history(bot_message(
            &receipt,
            Some(root),
            RELAY_PEER,
            "receipt: delivered",
        ));
    }
    fake.state()
        .scan_failures
        .push_back(TransportError::not_applied(
            "slack_rate_limited",
            "Slack rate-limited request conversations.replies. Retry after 60 seconds. \
             {\"retry_after_seconds\":60}",
            true,
        ));
    RELAY_HOLD_LIMITED_NOW.store(T0 + 48_000, Ordering::SeqCst);
    drain_batches(&mut subscription);
    let hold_scans = &fake.state().scans[tracked..];
    let thread_listings = hold_scans.iter().filter(|scan| scan.1.is_some()).count();
    assert_eq!(
        thread_listings, 1,
        "held listings stop at the first rate limit: {hold_scans:?}"
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn rate_limited_listing_logs_method_and_channel() {
    // #805: every rate-limited listing is logged with the Slack method
    // and channel, so the trigger can be found in the plugin logs.
    let directory = scratch("relay-ratelimit-log");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Idle; 10]);
    let root = ts(T0 + 10_000);
    fake.push_history(bot_message(&root, None, RELAY_PEER, "relay envelope"));
    fake.state()
        .scan_failures
        .push_back(TransportError::not_applied(
            "slack_rate_limited",
            "Slack rate-limited request conversations.history. Retry after 0 seconds. \
             {\"retry_after_seconds\":0}",
            true,
        ));
    let mut backend = backend(&fake, relay_log_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &relay_request(&directory, relay_extra())).unwrap();
    commit_next(&mut subscription);
    RELAY_LOG_NOW.store(T0 + 12_000, Ordering::SeqCst);
    let batch = next_batch_long(&mut subscription).expect("relay batch after the retry");
    subscription.commit_durable(&batch).unwrap();
    let logged = log(&directory);
    assert!(
        logged.contains("conversations.history") && logged.contains(CHANNEL),
        "rate-limit log line names method and channel: {logged}"
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(starve_clock, STARVE_NOW);

const OWNER_CHANNEL: &str = "C0OWNER1892";
const RELAY_CHANNEL: &str = "C0RELAY1892";

#[test]
fn a_rate_limited_relay_poll_never_starves_the_owner_rescan() {
    // #1892: a busy relay channel whose poll keeps hitting Slack's rate
    // limit must not hold the owner channel's rescan back: the relay poll
    // spends its own smaller budget, and each pass lists the owner channel
    // first, so the owner message is admitted within one pass.
    let directory = scratch("relay-starve");
    let fake = FakeTransport::new();
    fake.push_script(heartbeats(12));
    // A relay envelope the poll would carry, and an owner file message the
    // stream never carries: only the rescan can admit the owner's.
    let relay_ts = ts(T0 + 10_000);
    let mut envelope = listed(&relay_ts, None, RELAY_PEER, "relay envelope", &[]);
    envelope.channel_id = RELAY_CHANNEL.to_owned();
    envelope.subtype = Some("bot_message".to_owned());
    fake.push_history(envelope);
    let owner_ts = ts(T0 + 5_000);
    let mut owner_message = listed(&owner_ts, None, OWNER, "owner screenshot", &["F0KKKKKKKK1"]);
    owner_message.channel_id = OWNER_CHANNEL.to_owned();
    fake.push_history(owner_message);
    // The relay poll's history listing (and its in-pass retries) all hit
    // the rate limit, so the poll backs off in its own budget.
    for _ in 0..4 {
        fake.state()
            .scan_failures
            .push_back(TransportError::not_applied(
                "slack_rate_limited",
                "Slack rate-limited request conversations.history. Retry after 0 seconds. \
                 {\"retry_after_seconds\":0}",
                true,
            ));
    }
    let extra = json!({
        "relay_channels": [{"channel": RELAY_CHANNEL, "peer_senders": [RELAY_PEER]}],
        "relay_poll_seconds": 5,
        "rescan_budget_key": directory.to_str().unwrap(),
        "rescan_calls_per_minute": 20,
    });
    let request = SubscribeRequest::new(
        vec![
            ChannelId::new(OWNER_CHANNEL).unwrap(),
            ChannelId::new(RELAY_CHANNEL).unwrap(),
        ],
        vec![SenderId::new(OWNER).unwrap()],
        None,
    )
    .unwrap()
    .with_backend_configuration(
        BackendConfiguration::new("agentctl.slack.v1", data(&directory, extra)).unwrap(),
    );
    let mut backend = backend(&fake, starve_clock);
    let mut subscription = ChatSubscription::open(&mut backend, &request).unwrap();
    commit_next(&mut subscription);
    // Past the owner message's grace period: one pass must admit it even
    // though the relay poll just exhausted its listings on the rate limit.
    STARVE_NOW.store(T0 + 100_000, Ordering::SeqCst);
    let batch = next_batch_within(&mut subscription, 10).expect("owner batch within one pass");
    assert_eq!(
        single_message(&batch).message_id().as_str(),
        format!("{OWNER_CHANNEL}/messages/{owner_ts}")
    );
    subscription.commit_durable(&batch).unwrap();
    let logged = log(&directory);
    assert!(
        logged.contains(&format!(
            "listing rate-limited: conversations.history in channel {RELAY_CHANNEL}"
        )),
        "the relay listing hit the limit: {logged}"
    );
    assert!(
        !logged.contains("rescan: the host listing budget holds every bridge back"),
        "the owner rescan was never held back: {logged}"
    );
    // The back-off lives only in the relay budget; the owner budget is clean.
    let relay_state: Value =
        serde_json::from_slice(&fs::read(directory.join("budget-relay.json")).unwrap()).unwrap();
    assert!(
        relay_state["blocked_until_millis"].as_u64().unwrap_or(0) >= T0 + 100_000 + 30_000,
        "relay budget holds the back-off: {relay_state}"
    );
    let owner_state: Value =
        serde_json::from_slice(&fs::read(directory.join("budget.json")).unwrap()).unwrap();
    assert_eq!(
        owner_state["blocked_until_millis"].as_u64().unwrap_or(0),
        0,
        "owner budget was never blocked: {owner_state}"
    );
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

// #2366 slack-old-thread-reply-late: a reply the stream never carries, in a thread whose root is
// older than the rescan window, waited 5 h 20 min on a live bridge: no history row shows such a
// thread, and the stream had carried nothing in it for hours, so nothing listed it until the
// stream carried a later reply there.
const HOUR: u64 = 60 * 60 * 1_000;

fn watch_extra() -> Value {
    // A ten-minute window keeps the root out of history; no cursor refresh, so a jump of hours
    // on the test clock reopens nothing.
    json!({"rescan_window_seconds": 600, "cursor_refresh_seconds": 0})
}

fn watch_file(directory: &Path) -> Value {
    serde_json::from_slice(
        &fs::read(directory.join(chat_subscription_slack::watch::WATCH_FILE)).unwrap(),
    )
    .unwrap()
}

clock!(watched_clock, WATCHED_NOW);

#[test]
fn a_reply_in_a_watched_thread_older_than_the_window_is_rescued_hours_after_the_stream_went_quiet()
{
    let directory = scratch("watched");
    let fake = FakeTransport::new();
    let root = ts(T0 - 2 * HOUR);
    let carried_reply = ts(T0 + 5_000);
    let bridge_reply = ts(T0 + 3 * HOUR - 120_000);
    let question = ts(T0 + 3 * HOUR - 60_000);
    fake.push_history(listed(&root, None, OWNER, "old root", &[]));
    fake.push_history(listed(&carried_reply, Some(&root), OWNER, "a reply", &[]));
    let mut script = vec![stream_message(4, &carried_reply, Some(&root), "a reply")];
    script.extend(heartbeats(6));
    fake.push_script(script);
    let mut backend = backend(&fake, watched_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, watch_extra())).unwrap();
    commit_next(&mut subscription);
    commit_next(&mut subscription);
    let key = format!("{CHANNEL}/{root}");
    assert_eq!(watch_file(&directory)["threads"][0]["key"], key.as_str());

    // Three hours later, with nothing more on the stream: the bridge replied in the thread
    // (a bot post, never on the stream), and a minute later a reply the stream never carried.
    fake.push_history(listed(&bridge_reply, Some(&root), BOT, "our reply", &[]));
    fake.push_history(listed(&question, Some(&root), OWNER, "a question", &[]));
    WATCHED_NOW.store(T0 + 3 * HOUR, Ordering::SeqCst);
    let rescued = next_batch_within(&mut subscription, 3)
        .expect("the watched thread is listed and its reply rescued");
    let reply = single_message(&rescued);
    assert_eq!(
        reply.message_id().as_str(),
        format!("{CHANNEL}/messages/{question}")
    );
    assert_eq!(
        reply.thread_id().as_str(),
        format!("{CHANNEL}/threads/{root}")
    );
    subscription.commit_durable(&rescued).unwrap();
    let text = log(&directory);
    assert!(text.contains("; 1 watched thread listed"), "{text}");
    assert!(
        text.contains(&format!(
            "rescan: watched threads rescued {CHANNEL}/{question} 1 min after it was posted"
        )),
        "{text}"
    );
    let saved = watch_file(&directory);
    assert_eq!(
        saved["schema"],
        chat_subscription_slack::watch::WATCH_SCHEMA
    );
    assert_eq!(saved["threads"][0]["listed_at_millis"], T0 + 3 * HOUR);
    subscription.close().unwrap();
    fs::remove_dir_all(&directory).unwrap();
}

clock!(refused_clock, REFUSED_NOW);

/// The thread listings so far, as (thread root, oldest).
fn thread_listings(fake: &FakeTransport) -> Vec<(String, String)> {
    fake.state()
        .scans
        .iter()
        .filter_map(|scan| scan.1.clone().map(|root| (root, scan.2.clone())))
        .collect()
}

// While the bridge's request store is full it refuses every batch, and the provider opens a new
// subscription about once a minute. The watch must survive that: a reply that only the watch
// makes visible is still found, offered again identically after a refusal, and admitted once;
// and a new subscription neither lists a watched thread from the window floor again nor before
// its back-off.
#[test]
fn a_watched_thread_survives_refused_batches_and_resubscribes_without_relisting() {
    let directory = scratch("watched-refused");
    let fake = FakeTransport::new();
    let root = ts(T0 - 2 * HOUR);
    let carried_reply = ts(T0 + 5_000);
    let question = ts(T0 + 3 * HOUR - 60_000);
    fake.push_history(listed(&root, None, OWNER, "old root", &[]));
    fake.push_history(listed(&carried_reply, Some(&root), OWNER, "a reply", &[]));
    let mut script = vec![stream_message(4, &carried_reply, Some(&root), "a reply")];
    script.extend(heartbeats(2));
    fake.push_script(script);
    let mut backend = backend(&fake, refused_clock);
    let mut first =
        ChatSubscription::open(&mut backend, &request(&directory, None, watch_extra())).unwrap();
    commit_next(&mut first);
    commit_next(&mut first);
    first.close().unwrap();
    let resume = "slack1:seq:4";

    // Three hours later the reply arrives, and the store is full: the rescued batch is refused.
    fake.push_history(listed(&question, Some(&root), OWNER, "a question", &[]));
    REFUSED_NOW.store(T0 + 3 * HOUR, Ordering::SeqCst);
    fake.push_script(heartbeats(6));
    let mut refused = ChatSubscription::open(
        &mut backend,
        &request(&directory, Some(resume), watch_extra()),
    )
    .unwrap();
    let offered = next_batch_within(&mut refused, 3).expect("the watched thread's reply");
    assert_eq!(
        offered.cursor().as_str(),
        format!("{resume};rescued:{CHANNEL}/{question}")
    );
    refused.close().unwrap();
    let listings = thread_listings(&fake);
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].0, root);

    // The next subscription offers the same batch first, before any listing, and it is
    // admitted once.
    REFUSED_NOW.store(T0 + 3 * HOUR + 30_000, Ordering::SeqCst);
    fake.push_script(heartbeats(6));
    let mut admitted = ChatSubscription::open(
        &mut backend,
        &request(&directory, Some(resume), watch_extra()),
    )
    .unwrap();
    let again = next_batch_within(&mut admitted, 1).expect("offered again first");
    assert_eq!(again.cursor(), offered.cursor());
    assert_eq!(again.events(), offered.events());
    assert_eq!(
        thread_listings(&fake).len(),
        1,
        "nothing was listed before the replay"
    );
    admitted.commit_durable(&again).unwrap();
    // Its newest message is a minute old, so the thread is listed on this pass too, from where
    // it was listed through (the reply), not from the window floor; nothing is offered twice.
    assert!(next_batch_within(&mut admitted, 2).is_none());
    let listings = thread_listings(&fake);
    assert_eq!(listings.len(), 2);
    assert_eq!(listings[1], (root.clone(), question.clone()));

    // An hour later the thread is quiet: one pass lists it, and a new subscription three
    // minutes later does not list it again; ten minutes after the last listing it is due.
    REFUSED_NOW.store(T0 + 4 * HOUR + 30_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut admitted, 2).is_none());
    assert_eq!(thread_listings(&fake).len(), 3);
    admitted.close().unwrap();
    REFUSED_NOW.store(T0 + 4 * HOUR + 210_000, Ordering::SeqCst);
    fake.push_script(heartbeats(6));
    let mut later = ChatSubscription::open(
        &mut backend,
        &request(&directory, Some(resume), watch_extra()),
    )
    .unwrap();
    assert!(next_batch_within(&mut later, 2).is_none());
    assert_eq!(
        thread_listings(&fake).len(),
        3,
        "a quiet watched thread waits out its back-off across subscriptions"
    );
    REFUSED_NOW.store(T0 + 4 * HOUR + 30_000 + 10 * 60_000, Ordering::SeqCst);
    assert!(next_batch_within(&mut later, 2).is_none());
    let listings = thread_listings(&fake);
    assert_eq!(listings.len(), 4);
    assert_eq!(listings[3].0, root);
    later.close().unwrap();
    let saved = ledger(&directory);
    assert_eq!(saved["rescued"].as_array().unwrap().len(), 1);
    assert_eq!(saved["pending"], Value::Null);
    fs::remove_dir_all(&directory).unwrap();
}

clock!(watch_off_clock, WATCH_OFF_NOW);

#[test]
fn the_watch_can_be_turned_off_and_a_too_short_one_is_refused() {
    let directory = scratch("watch-off");
    let fake = FakeTransport::new();
    let root = ts(T0 - 2 * HOUR);
    let carried_reply = ts(T0 + 5_000);
    let mut script = vec![stream_message(4, &carried_reply, Some(&root), "a reply")];
    script.extend(heartbeats(2));
    fake.push_script(script);
    let mut backend = backend(&fake, watch_off_clock);
    let mut off = json!({"rescan_watch_seconds": 0});
    off.as_object_mut()
        .unwrap()
        .extend(watch_extra().as_object().unwrap().clone());
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, None, off)).unwrap();
    commit_next(&mut subscription);
    commit_next(&mut subscription);
    subscription.close().unwrap();
    assert!(!directory
        .join(chat_subscription_slack::watch::WATCH_FILE)
        .exists());
    let refused = ChatSubscription::open(
        &mut backend,
        &request(&directory, None, json!({"rescan_watch_seconds": 1_800})),
    );
    assert!(refused.is_err());
    fs::remove_dir_all(&directory).unwrap();
}

clock!(invalid_watch_clock, INVALID_WATCH_NOW);

// A consumer pins old readers and asked that a damaged watch file never stop the bridge: garbage
// or an unknown schema is an empty watch and a log line, and the next watched thread replaces
// the file with a valid one.
#[test]
fn a_garbage_or_unknown_schema_watch_file_never_stops_the_subscription() {
    for (name, bytes) in [
        ("garbage", b"\x00\xff{{ not json".as_slice()),
        (
            "schema",
            br#"{"schema":"agentctl.slack.watched-threads.v2","threads":[]}"#.as_slice(),
        ),
    ] {
        let directory = scratch(&format!("invalid-watch-{name}"));
        let file = directory.join(chat_subscription_slack::watch::WATCH_FILE);
        fs::write(&file, bytes).unwrap();
        let fake = FakeTransport::new();
        let root = ts(T0 - 2 * HOUR);
        let carried_reply = ts(T0 + 5_000);
        let mut script = vec![stream_message(4, &carried_reply, Some(&root), "a reply")];
        script.extend(heartbeats(2));
        fake.push_script(script);
        let mut backend = backend(&fake, invalid_watch_clock);
        let mut subscription =
            ChatSubscription::open(&mut backend, &request(&directory, None, watch_extra()))
                .expect("a damaged watch file never stops the subscription");
        commit_next(&mut subscription);
        let delivered = commit_next(&mut subscription);
        assert_eq!(
            single_message(&delivered).message_id().as_str(),
            format!("{CHANNEL}/messages/{carried_reply}")
        );
        subscription.close().unwrap();
        let text = log(&directory);
        assert!(
            text.contains(
                "rescan: the watched-thread file was not a valid watch; starting an empty one"
            ),
            "{name}: {text}"
        );
        assert!(
            text.contains("rescan: watching 0 threads"),
            "{name}: {text}"
        );
        let saved = watch_file(&directory);
        assert_eq!(
            saved["schema"],
            chat_subscription_slack::watch::WATCH_SCHEMA
        );
        assert_eq!(
            saved["threads"][0]["key"],
            format!("{CHANNEL}/{root}").as_str()
        );
        fs::remove_dir_all(&directory).unwrap();
    }
}

clock!(other_cursor_clock, OTHER_CURSOR_NOW);

#[test]
fn an_unacknowledged_rescue_under_an_unrelated_cursor_is_offered_again_not_dropped() {
    // #2430 S1: a rescued message the bridge never acknowledged, when the next subscription
    // resumes from a cursor that is neither the rescue's nor the one before it, was recorded
    // delivered and never offered again: lost, with only a diagnostics line. It is now offered
    // again from the new subscription's cursor; the bridge dedupes by message key.
    let directory = scratch("other-cursor");
    let fake = FakeTransport::new();
    let first_ts = ts(T0 + 10_000);
    fake.push_history(listed(&first_ts, None, OWNER, "first", &["F0DDDDDDDD1"]));
    OTHER_CURSOR_NOW.store(T0 + 100_000, Ordering::SeqCst);
    fake.push_script(heartbeats(4));
    let resume = "slack1:seq:9";
    let mut backend = backend(&fake, other_cursor_clock);
    {
        let (mut ledger, _) = chat_subscription_slack::rescan::Ledger::load(
            directory.join(LEDGER_FILE),
            u128::from(T0) * 1_000,
        );
        ledger.cursor = Some(resume.to_owned());
        ledger.save().unwrap();
    }
    let mut subscription =
        ChatSubscription::open(&mut backend, &request(&directory, Some(resume), json!({})))
            .unwrap();
    let offered = next_batch_within(&mut subscription, 1).expect("first rescue");
    assert!(offered
        .cursor()
        .as_str()
        .ends_with(&format!(";rescued:{CHANNEL}/{first_ts}")));
    // Never acknowledged; the bridge comes back from some other cursor.
    subscription.close().unwrap();
    let elsewhere = "slack1:seq:12";
    fake.push_script(heartbeats(4));
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(&directory, Some(elsewhere), json!({})),
    )
    .unwrap();
    let again = next_batch_within(&mut subscription, 2).expect("offered again, not dropped");
    assert_eq!(
        again.cursor().as_str(),
        format!("{elsewhere};rescued:{CHANNEL}/{first_ts}"),
        "built on the new subscription's cursor"
    );
    assert_eq!(again.events(), offered.events(), "the same message");
    subscription.commit_durable(&again).unwrap();
    subscription.close().unwrap();
    let saved = ledger(&directory);
    assert_eq!(saved["pending"], Value::Null);
    assert!(!log(&directory).contains("recorded delivered and not offered again"));
}

clock!(gap_clock, GAP_NOW);

#[test]
fn a_provider_gap_never_halts_the_bridge_and_the_rescan_fills_its_window_across_a_restart() {
    // #2440 D6: the tail reports lost events. The plugin offers one gap batch at the far side
    // (a time cursor taken when the gap was seen), the bridge commits it and keeps running, and
    // the message sent inside the gap, which the stream never carried, arrives once through the
    // rescan, even after a restart that resumes from the far side (a time cursor otherwise
    // refuses anything created before it).
    let directory = scratch("gap");
    let root = directory.join("bridge");
    let state = host_state(&root, &directory);
    let fake = FakeTransport::new();
    let carried = ts(T0 + 5_000);
    let in_gap = ts(T0 + 30_000);
    let mut script = vec![stream_message(20, &carried, None, "before the gap")];
    script.extend(heartbeats(2));
    script.push(FakeStep::Line(
        r#"{"type":"GAP","lost_from_iris_seq_id":21,"lost_to_iris_seq_id":40,"reason":"retention_overflow"}"#
            .to_owned(),
    ));
    script.extend(heartbeats(3));
    fake.push_script(script);
    fake.push_history(listed(&carried, None, OWNER, "before the gap", &[]));
    fake.push_history(listed(&in_gap, None, OWNER, "sent during the gap", &[]));
    let mut backend = backend(&fake, gap_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &state.subscribe_request().unwrap()).unwrap();
    let mut keys = drain(&mut subscription, &state, 2);
    assert_eq!(keys.len(), 1, "the carried message");
    // The gap is seen a minute in, after the message inside it was sent.
    let seen = T0 + 60_000;
    GAP_NOW.store(seen, Ordering::SeqCst);
    keys.extend(drain(&mut subscription, &state, 3));
    assert_eq!(keys.len(), 1, "the gap carries no message");
    let far_side = format!("slack1:since:{seen}");
    assert_eq!(state.cursor().unwrap().as_deref(), Some(far_side.as_str()));
    let reconcile = state
        .gap_reconcile()
        .expect("the bridge opened a reconcile");
    assert_eq!(reconcile.proposed_cursor, far_side);
    assert!(
        reconcile.reasons[0].contains("event stream lost events from sequence 21 to 40")
            && reconcile.reasons[0].contains("the rescan admits the window's messages"),
        "{:?}",
        reconcile.reasons
    );
    let saved = ledger(&directory);
    assert_eq!(saved["cursor"], far_side.as_str());
    assert_eq!(saved["gap_cursor"], far_side.as_str());
    subscription.close().unwrap();

    // Restart from the far side: the rescan still reaches back for the gap's message.
    let reopened = BridgeState::open(&root).unwrap();
    fake.push_script(heartbeats(6));
    GAP_NOW.store(T0 + 200_000, Ordering::SeqCst);
    let request = reopened.subscribe_request().unwrap();
    assert_eq!(
        request.resume_from().map(ProviderCursor::as_str),
        Some(far_side.as_str())
    );
    let mut subscription = ChatSubscription::open(&mut backend, &request).unwrap();
    let rescued = drain(&mut subscription, &reopened, 4);
    assert_eq!(rescued.len(), 1, "the message sent inside the gap, once");
    keys.extend(rescued);
    // Later passes offer it no more.
    GAP_NOW.store(T0 + 400_000, Ordering::SeqCst);
    assert!(drain(&mut subscription, &reopened, 4).is_empty());
    subscription.close().unwrap();
    let mut requests = reopened.request_keys().unwrap();
    requests.sort();
    keys.sort();
    assert_eq!(requests, keys);
    assert!(keys.iter().any(|key| {
        fs::read_to_string(root.join("requests").join(format!("{key}.json")))
            .unwrap()
            .contains("sent during the gap")
    }));
    assert!(log(&directory).contains("resuming from the far side of a reported gap"));
    fs::remove_dir_all(&directory).unwrap();
}

clock!(gap_ack_clock, GAP_ACK_NOW);

#[test]
fn a_gap_committed_without_its_acknowledgement_still_resumes_as_known() {
    // The bridge may commit the gap batch and lose the acknowledgement on the way back. The far
    // side is the plugin's own cursor and nothing was offered after it, so a resume from it keeps
    // the ledger's start, and the rescan can still reach back into the gap (#2440 D6).
    let directory = scratch("gap-ack");
    let fake = FakeTransport::new();
    fake.push_script(vec![FakeStep::Line(
        r#"{"type":"GAP","lost_from_iris_seq_id":21,"lost_to_iris_seq_id":40,"reason":"retention_overflow"}"#
            .to_owned(),
    )]);
    let mut backend = backend(&fake, gap_ack_clock);
    let mut subscription = ChatSubscription::open(
        &mut backend,
        &request(&directory, Some("slack1:seq:20"), json!({})),
    )
    .unwrap();
    let gap = next_batch_within(&mut subscription, 1).expect("the gap");
    let tracking_since = ledger(&directory)["tracking_since_micros"].clone();
    let far_side = format!("slack1:since:{T0}");
    assert_eq!(gap.cursor().as_str(), far_side);
    assert_eq!(ledger(&directory)["gap_cursor"], far_side.as_str());
    // No acknowledgement: the subscription ends with the gap still pending.
    drop(subscription);
    GAP_ACK_NOW.store(T0 + 60_000, Ordering::SeqCst);
    fake.push_script(heartbeats(2));
    let subscription = ChatSubscription::open(
        &mut backend,
        &request(&directory, Some(&far_side), json!({})),
    )
    .unwrap();
    assert_eq!(ledger(&directory)["tracking_since_micros"], tracking_since);
    assert!(
        !log(&directory).contains("not from the ledger's last acknowledged cursor"),
        "{}",
        log(&directory)
    );
    drop(subscription);
    fs::remove_dir_all(&directory).unwrap();
}

clock!(once_clock, ONCE_NOW);

/// The request records whose saved text holds `needle`.
fn requests_holding(root: &Path, state: &BridgeState, needle: &str) -> Vec<String> {
    state
        .request_keys()
        .unwrap()
        .into_iter()
        .filter(|key| {
            fs::read_to_string(root.join("requests").join(format!("{key}.json")))
                .unwrap()
                .contains(needle)
        })
        .collect()
}

#[test]
fn an_owner_message_sent_inside_a_gap_is_delivered_exactly_once_across_restarts() {
    // #2440 D5/D6 (lead-014's condition): an owner message posted inside a gap's window reaches
    // the bridge EXACTLY ONCE after the reconcile: never lost, never twice, through a restart
    // that resumes from the gap's far side, the stream carrying a late copy, and a second
    // restart whose rescan lists the message again.
    let directory = scratch("gap-once");
    let root = directory.join("bridge");
    let state = host_state(&root, &directory);
    let fake = FakeTransport::new();
    let before = ts(T0 + 5_000);
    let inside = ts(T0 + 30_000);
    let mut script = vec![stream_message(20, &before, None, "before the gap")];
    script.extend(heartbeats(2));
    script.push(FakeStep::Line(
        r#"{"type":"GAP","lost_from_iris_seq_id":21,"lost_to_iris_seq_id":40,"reason":"retention_overflow"}"#
            .to_owned(),
    ));
    script.extend(heartbeats(3));
    fake.push_script(script);
    fake.push_history(listed(&before, None, OWNER, "before the gap", &[]));
    fake.push_history(listed(&inside, None, OWNER, "posted inside the gap", &[]));
    let mut backend = backend(&fake, once_clock);
    let mut subscription =
        ChatSubscription::open(&mut backend, &state.subscribe_request().unwrap()).unwrap();
    drain(&mut subscription, &state, 2);
    let seen = T0 + 60_000;
    ONCE_NOW.store(seen, Ordering::SeqCst);
    drain(&mut subscription, &state, 3);
    let far_side = format!("slack1:since:{seen}");
    assert_eq!(state.cursor().unwrap().as_deref(), Some(far_side.as_str()));
    assert!(requests_holding(&root, &state, "posted inside the gap").is_empty());
    subscription.close().unwrap();

    // Restart 1, from the far side: the rescan delivers the message; then the stream carries a
    // late copy of it, which is not a second request.
    let first = BridgeState::open(&root).unwrap();
    let mut script = heartbeats(3);
    script.push(stream_message(41, &inside, None, "posted inside the gap"));
    script.extend(heartbeats(4));
    fake.push_script(script);
    ONCE_NOW.store(T0 + 200_000, Ordering::SeqCst);
    let request = first.subscribe_request().unwrap();
    assert_eq!(
        request.resume_from().map(ProviderCursor::as_str),
        Some(far_side.as_str())
    );
    let mut subscription = ChatSubscription::open(&mut backend, &request).unwrap();
    let new_keys = drain(&mut subscription, &first, 4);
    let delivered = requests_holding(&root, &first, "posted inside the gap");
    assert_eq!(delivered.len(), 1, "delivered, once");
    assert_eq!(
        new_keys, delivered,
        "the only new request is the gap's message"
    );
    subscription.close().unwrap();

    // Restart 2, from wherever the bridge now is: the rescan lists the message again and the
    // stream replays nothing new; nothing is admitted twice.
    let second = BridgeState::open(&root).unwrap();
    fake.push_script(heartbeats(6));
    ONCE_NOW.store(T0 + 500_000, Ordering::SeqCst);
    let mut subscription =
        ChatSubscription::open(&mut backend, &second.subscribe_request().unwrap()).unwrap();
    assert!(drain(&mut subscription, &second, 4).is_empty());
    subscription.close().unwrap();
    assert_eq!(
        requests_holding(&root, &second, "posted inside the gap"),
        delivered,
        "still one request, the same one"
    );
    assert_eq!(
        second.request_keys().unwrap().len(),
        2,
        "before the gap, inside it"
    );
    // Exactly one prompt is owed for it: the request waits once for delivery to the agent.
    let pending = second.pending_work_keys().unwrap();
    assert_eq!(
        pending.iter().filter(|key| delivered.contains(key)).count(),
        1,
        "{pending:?}"
    );
    fs::remove_dir_all(&directory).unwrap();
}
