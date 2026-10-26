//! The Slack provider speaks the bridge's own wire formats: requests encoded by the bridge are
//! accepted by the Slack outbound helper, its responses decode with the bridge's strict decoders,
//! and a bridge configuration for a Slack channel yields a subscription request the Slack plugin
//! accepts.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use agentctl::chat_runtime::{
    decode_reaction_response, decode_rejection_response, decode_root_message_response,
    decode_send_response, encode_reaction_request, encode_rejection_request,
    encode_root_message_request, encode_send_request, BackendConfigurationDocument,
    BridgeConfiguration, BridgeState, OutboundOutcome, ReactionReceipt, ReactionSetting,
    ReactionSubmission, RejectionSubmission, ReplySubmission, RootMessageSubmission,
};
use chat_subscription::{
    ChannelId, CommittableEvent, DeliveryBatch, DeliveryId, EventSequence, MessageId,
    ProviderCursor, RejectedMessage, RejectionReason, SenderId,
};
use chat_subscription_slack::fake::FakeTransport;
use chat_subscription_slack::outbound::{handle_request, OutboundConfig};
use chat_subscription_slack::plugin::PluginConfig;
use chat_subscription_slack::transport::SlackTransport;
use serde_json::{json, Map, Value};

const CHANNEL: &str = "C0C91RW5FAR";

fn helper(app: Option<&str>) -> OutboundConfig {
    let ledger = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("chat-slack-wire-{}", app.unwrap_or("none")));
    let _ = std::fs::remove_dir_all(&ledger);
    let mut arguments = vec![
        "--cli=/opt/slack/bin/slack-cli".to_owned(),
        format!("--channel={CHANNEL}"),
        format!("--ledger={}", ledger.display()),
    ];
    if let Some(app) = app {
        arguments.push(format!("--app-id={app}"));
    }
    OutboundConfig::parse(&arguments).expect("helper arguments")
}

fn exchange(config: &OutboundConfig, fake: &FakeTransport, request: &[u8]) -> Vec<u8> {
    let mut transport = || Ok(Box::new(fake.clone()) as Box<dyn SlackTransport>);
    handle_request(request, Ok(config), &mut transport, &|| 1_000)
}

#[test]
fn bridge_requests_round_trip_through_the_slack_helper() {
    let config = helper(Some("A0TESTAPP01"));
    let fake = FakeTransport::new();

    let thread = format!("{CHANNEL}/threads/1792000801.000100");
    let reply = ReplySubmission {
        channel_id: CHANNEL,
        thread_id: &thread,
        body: "[coordinator] done",
        request_id: "0b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
    };
    let response = exchange(&config, &fake, &encode_send_request(&reply).unwrap());
    let message_id = decode_send_response(&response, &reply).expect("bridge accepts the receipt");
    assert!(message_id.starts_with("C0C91RW5FAR/messages/"));
    assert_eq!(
        fake.state().posts[0].thread_ts.as_deref(),
        Some("1792000801.000100")
    );

    let root = RootMessageSubmission {
        channel_id: CHANNEL,
        body: "status: green",
        request_id: "2b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
    };
    let response = exchange(&config, &fake, &encode_root_message_request(&root).unwrap());
    decode_root_message_response(&response, &root).expect("bridge accepts the root receipt");
    assert_eq!(fake.state().posts[1].thread_ts, None);

    let message = format!("{CHANNEL}/messages/1792000801.000100");
    for emoji in ["🤖", "✅"] {
        let reaction = ReactionSubmission {
            channel_id: CHANNEL,
            message_id: &message,
            emoji,
            request_id: "1b7e4f6a-1c2d-4e5f-9a9b-0c1d2e3f4a5b",
        };
        let response = exchange(&config, &fake, &encode_reaction_request(&reaction).unwrap());
        let receipt =
            decode_reaction_response(&response, &reaction).expect("bridge accepts the reaction");
        assert!(receipt
            .reaction_id
            .starts_with(&format!("{message}/reactions/")));
    }
    let names: Vec<String> = fake
        .state()
        .reactions
        .iter()
        .map(|reaction| reaction.name.clone())
        .collect();
    assert_eq!(names, ["robot_face", "white_check_mark"]);
    assert!(fake
        .state()
        .posts
        .iter()
        .all(|post| post.app_id == "A0TESTAPP01"));
}

#[test]
fn a_refusal_without_an_app_decodes_as_not_applied() {
    let config = helper(None);
    let fake = FakeTransport::new();
    let thread = format!("{CHANNEL}/threads/1792000801.000100");
    let reply = ReplySubmission {
        channel_id: CHANNEL,
        thread_id: &thread,
        body: "hello",
        request_id: "0b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
    };
    let response = exchange(&config, &fake, &encode_send_request(&reply).unwrap());
    let failure = decode_send_response(&response, &reply).expect_err("refused");
    assert_eq!(failure.code, "app_identity_required");
    assert_eq!(failure.outcome, OutboundOutcome::NotApplied);
    assert!(fake.state().posts.is_empty());
}

#[test]
fn guarded_rejection_round_trips_the_bridges_exact_protocol_and_refusal() {
    let config = helper(Some("A0REJECT001"));
    let fake = FakeTransport::new();
    let message = format!("{CHANNEL}/messages/1792000801.000100");
    let human = RejectionSubmission {
        channel_id: CHANNEL,
        message_id: &message,
        sender_id: "U0GUEST001",
        emoji: "❌",
        request_id: "3b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
    };
    fake.state()
        .sender_humanity
        .insert(human.sender_id.to_owned(), true);
    let response = exchange(&config, &fake, &encode_rejection_request(&human).unwrap());
    let receipt = decode_rejection_response(&response, &human)
        .expect("bridge accepts guarded reaction receipt");
    assert_eq!(receipt.reaction_id, format!("{message}/reactions/x"));
    assert!(!receipt.already_present);
    assert_eq!(fake.state().reactions.len(), 1);

    let bot = RejectionSubmission {
        sender_id: "U0BRIDGEBOT",
        request_id: "4b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
        ..human
    };
    fake.state()
        .sender_humanity
        .insert(bot.sender_id.to_owned(), false);
    let response = exchange(&config, &fake, &encode_rejection_request(&bot).unwrap());
    let failure = decode_rejection_response(&response, &bot)
        .expect_err("bridge retains authoritative bot refusal");
    assert_eq!(failure.code, "author_not_human");
    assert_eq!(failure.outcome, OutboundOutcome::NotApplied);
    assert!(!failure.retryable);
    assert_eq!(
        fake.state().reactions.len(),
        1,
        "bot refusal applies no second reaction"
    );
}

/// An idle scratch bridge: only this test reads or edits its private documents. Completion is
/// installed as a fixture because the runtime's rejection worker is crate-private; both helper
/// effects and receipts still go through the real wire encoder, handler and decoder.
struct RejectionReplayFixture {
    root: PathBuf,
    state: BridgeState,
}

impl RejectionReplayFixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "chat-slack-rejection-replay-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let configuration = serde_json::from_value(json!({
            "subscription_plugin": "slack",
            "channel_ids": [CHANNEL],
            "allowed_senders": ["U0OWNER0001"],
            "agent_name": "coordinator",
            "agent_label": "test coordinator",
            "ack_reaction": null,
            "receipt_reaction": null,
            "reject_reaction": "❌",
        }))
        .expect("bridge configuration");
        let state = BridgeState::initialize(&root, configuration).expect("initialize bridge");
        Self { root, state }
    }

    fn document(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root.join("rejection-reactions.json")).unwrap())
            .expect("rejection outbox")
    }

    fn record(&self, key: &str) -> Value {
        self.document()["records"]
            .as_array()
            .expect("outbox records")
            .iter()
            .find(|record| record["key"] == key)
            .expect("saved rejection")
            .clone()
    }

    fn update_document(&self, name: &str, update: impl FnOnce(&mut Value)) {
        let path = self.root.join(name);
        let mut document =
            serde_json::from_slice(&fs::read(&path).expect("private document")).unwrap();
        update(&mut document);
        let temporary = self.root.join(format!(".{name}.fixture"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .expect("private fixture replacement");
        file.write_all(&serde_json::to_vec_pretty(&document).unwrap())
            .expect("write fixture replacement");
        file.sync_all().expect("sync fixture replacement");
        fs::rename(temporary, path).expect("install fixture replacement");
    }

    fn complete(&self, key: &str, receipt: &ReactionReceipt) {
        self.update_document("rejection-reactions.json", |document| {
            let record = document["records"]
                .as_array_mut()
                .expect("outbox records")
                .iter_mut()
                .find(|record| record["key"] == key)
                .expect("pending operation");
            assert_eq!(record["phase"], "pending");
            record["phase"] = json!("complete");
            record["attempted_at_millis"] = record["recorded_at_millis"].clone();
            record["reaction_id"] = json!(receipt.reaction_id);
            record["already_present"] = json!(receipt.already_present);
        });
    }
}

impl Drop for RejectionReplayFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn rejection_batch(sequence: u64, indices: impl IntoIterator<Item = u64>) -> DeliveryBatch {
    let events = indices
        .into_iter()
        .map(|index| {
            CommittableEvent::message_rejected(RejectedMessage::new(
                ChannelId::new(CHANNEL).expect("channel"),
                MessageId::new(format!(
                    "{CHANNEL}/messages/{}.000100",
                    1_792_000_801 + index
                ))
                .expect("message"),
                SenderId::new("U0GUEST001").expect("human sender"),
                RejectionReason::SenderNotAllowed,
            ))
        })
        .collect();
    DeliveryBatch::new(
        EventSequence::new(sequence).expect("sequence"),
        ProviderCursor::new(format!("slack1:seq:{sequence}")).expect("cursor"),
        DeliveryId::new(format!("slack-replay-{sequence}")).expect("delivery"),
        events,
    )
    .expect("rejection batch")
}

fn saved_rejection(record: &Value) -> RejectionSubmission<'_> {
    RejectionSubmission {
        channel_id: record["channel_id"].as_str().expect("saved channel"),
        message_id: record["message_id"].as_str().expect("saved message"),
        sender_id: record["sender_id"].as_str().expect("saved sender"),
        emoji: record["emoji"].as_str().expect("saved emoji"),
        request_id: record["request_id"].as_str().expect("saved operation"),
    }
}

fn ledger_operations(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => panic!("operation ledger: {error}"),
    };
    let mut files = entries
        .map(|entry| {
            let path = entry.expect("ledger entry").path();
            let bytes = fs::read(&path).expect("operation journal bytes");
            (path, bytes)
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[test]
fn replay_after_host_eviction_and_emoji_change_keeps_one_slack_rejection() {
    let helper = helper(Some("A0EVICT001"));
    let fake = FakeTransport::new();
    fake.state()
        .sender_humanity
        .insert("U0GUEST001".to_owned(), true);
    let fixture = RejectionReplayFixture::new();
    let full = fixture
        .state
        .admit_batch(&rejection_batch(1, 0..256))
        .expect("fill the bounded host outbox");
    assert_eq!(full.rejection_saves.saved.len(), 256);
    assert!(full.new_request_keys.is_empty());
    assert!(full.rejection_saves.errors.is_empty());
    assert_eq!(full.rejection_saves.dropped, 0);
    let first_key = &full.rejection_saves.saved[0];
    let first = fixture.record(first_key);
    let first_submission = saved_rejection(&first);
    assert_eq!(first_submission.channel_id, CHANNEL);
    assert_eq!(
        first_submission.message_id,
        format!("{CHANNEL}/messages/1792000801.000100")
    );
    assert_eq!(first_submission.sender_id, "U0GUEST001");
    assert_eq!(first_submission.emoji, "❌");
    let response = exchange(
        &helper,
        &fake,
        &encode_rejection_request(&first_submission).unwrap(),
    );
    let first_receipt =
        decode_rejection_response(&response, &first_submission).expect("first Slack receipt");
    assert_eq!(
        first_receipt.reaction_id,
        format!("{}/reactions/x", first_submission.message_id)
    );
    assert!(!first_receipt.already_present);
    fixture.complete(first_key, &first_receipt);

    let replacement = fixture
        .state
        .admit_batch(&rejection_batch(2, [256]))
        .expect("a new message evicts the completed host tombstone");
    assert_eq!(replacement.rejection_saves.saved.len(), 1);
    assert!(replacement.rejection_saves.errors.is_empty());
    assert_eq!(replacement.rejection_saves.dropped, 0);
    let document = fixture.document();
    assert_eq!(document["records"].as_array().unwrap().len(), 256);
    assert!(!document["records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|record| record["key"] == *first_key));

    // Complete the replacement through Slack too, making room for replay without fabricating
    // a provider receipt or deleting one of the still-pending host operations.
    let replacement_key = &replacement.rejection_saves.saved[0];
    let replacement_record = fixture.record(replacement_key);
    let replacement_submission = saved_rejection(&replacement_record);
    assert_ne!(
        replacement_submission.request_id,
        first_submission.request_id
    );
    let response = exchange(
        &helper,
        &fake,
        &encode_rejection_request(&replacement_submission).unwrap(),
    );
    let replacement_receipt = decode_rejection_response(&response, &replacement_submission)
        .expect("replacement Slack receipt");
    fixture.complete(replacement_key, &replacement_receipt);
    assert_eq!(fake.state().reactions.len(), 2);
    let ledger = helper
        .ledger
        .as_ref()
        .expect("durable helper ledger")
        .join("rejections");
    let operations_before_replay = ledger_operations(&ledger);

    fixture.update_document("bridge.json", |document| {
        document["config"]["reject_reaction"] = json!("no_entry_sign");
    });
    let reopened = BridgeState::open(&fixture.root).expect("restart with the changed emoji");
    assert_eq!(reopened.config().reject_emoji(), Some("no_entry_sign"));
    let replay = reopened
        .admit_batch(&rejection_batch(3, [0]))
        .expect("the same message can recur at a later provider cursor");
    assert_eq!(
        replay.rejection_saves.saved.as_slice(),
        std::slice::from_ref(first_key)
    );
    assert!(replay.new_request_keys.is_empty());
    assert!(replay.rejection_saves.errors.is_empty());
    assert_eq!(replay.rejection_saves.dropped, 0);
    let reconstructed = fixture.record(first_key);
    let reconstructed_submission = saved_rejection(&reconstructed);
    assert_eq!(
        reconstructed_submission.channel_id,
        first_submission.channel_id
    );
    assert_eq!(
        reconstructed_submission.message_id,
        first_submission.message_id
    );
    assert_eq!(
        reconstructed_submission.sender_id,
        first_submission.sender_id
    );
    assert_eq!(
        reconstructed_submission.request_id,
        first_submission.request_id
    );
    assert_eq!(reconstructed_submission.emoji, "no_entry_sign");
    assert_eq!(reconstructed["phase"], "pending");

    // FakeTransport models Slack's real app/channel/message/emoji deduplication; it has no
    // UUID deduplication. The actual helper must keep the original marker and operation.
    let response = exchange(
        &helper,
        &fake,
        &encode_rejection_request(&reconstructed_submission).unwrap(),
    );
    let replay_receipt = decode_rejection_response(&response, &reconstructed_submission)
        .expect("the helper recognizes the operation after its host tombstone was evicted");
    assert_eq!(
        replay_receipt.reaction_id, first_receipt.reaction_id,
        "an evicted host row must not change an existing Slack rejection's emoji"
    );
    assert!(replay_receipt.already_present);
    fixture.complete(first_key, &replay_receipt);
    let state = fake.state();
    assert_eq!(state.reactions.len(), 2, "replay adds no Slack reaction");
    let original_reactions = state
        .reactions
        .iter()
        .filter(|reaction| reaction.message_ts == "1792000801.000100")
        .collect::<Vec<_>>();
    assert_eq!(original_reactions.len(), 1);
    assert_eq!(original_reactions[0].app_id, "A0EVICT001");
    assert_eq!(original_reactions[0].channel_id, CHANNEL);
    assert_eq!(original_reactions[0].name, "x");
    assert!(state.posts.is_empty());
    assert_eq!(ledger_operations(&ledger), operations_before_replay);
    let journal_paths = operations_before_replay
        .iter()
        .map(|(path, _bytes)| path.clone())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    assert_eq!(journal_paths.len(), 2, "one journal per actual operation");
    assert!(journal_paths.contains(&ledger.join(format!("{}.json", first_submission.request_id))));
    assert!(
        journal_paths.contains(&ledger.join(format!("{}.json", replacement_submission.request_id)))
    );
    assert!(reopened.pending_request_keys().unwrap().is_empty());
    assert_eq!(
        fs::read_dir(fixture.root.join("requests")).unwrap().count(),
        0
    );
    assert_eq!(reopened.cursor().unwrap().as_deref(), Some("slack1:seq:3"));
    assert_eq!(fixture.document()["records"].as_array().unwrap().len(), 256);
}

#[test]
fn a_bridge_configuration_for_slack_is_accepted_by_the_plugin() {
    let configuration = BridgeConfiguration {
        subscription_plugin: "slack".to_owned(),
        subscription_environment: Vec::new(),
        channel_ids: vec![CHANNEL.to_owned()],
        allowed_senders: vec!["U0OWNER0001".to_owned()],
        max_grant_seconds: agentctl::chat_runtime::DEFAULT_MAX_GRANT_SECONDS,
        agent_name: "slack-test".to_owned(),
        agent_label: "slack test agent".to_owned(),
        outbound_enabled: true,
        ack_reaction: ReactionSetting::Emoji("🤖".to_owned()),
        receipt_reaction: ReactionSetting::Inherit,
        reject_reaction: ReactionSetting::Inherit,
        backend_configuration: Some(BackendConfigurationDocument {
            schema: "agentctl.slack.v1".to_owned(),
            data: Map::from_iter([
                ("cli_path".to_owned(), json!("/opt/slack/bin/slack-cli")),
                (
                    "ignore_text_prefixes".to_owned(),
                    json!(["[slack test agent]"]),
                ),
            ]),
        }),
        outbound_command: None,
        reply_alias_range: None,
        slack_format: Some(agentctl::chat_runtime::SlackFormat::Markdown),
        slack_format_by_channel: std::collections::BTreeMap::new(),
    };
    let request = configuration
        .subscribe_request(Some("slack1:seq:42"))
        .expect("the bridge builds the request");
    let parsed = PluginConfig::from_request(&request).expect("the plugin accepts it");
    assert_eq!(parsed.ignore_text_prefixes, ["[slack test agent]"]);
}
