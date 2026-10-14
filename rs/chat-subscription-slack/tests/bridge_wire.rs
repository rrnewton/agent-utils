//! The Slack provider speaks the bridge's own wire formats: requests encoded by the bridge are
//! accepted by the Slack outbound helper, its responses decode with the bridge's strict decoders,
//! and a bridge configuration for a Slack channel yields a subscription request the Slack plugin
//! accepts.

use agentctl::chat_runtime::{
    decode_reaction_response, decode_root_message_response, decode_send_response,
    encode_reaction_request, encode_root_message_request, encode_send_request,
    BackendConfigurationDocument, BridgeConfiguration, OutboundOutcome, ReactionSetting,
    ReactionSubmission, ReplySubmission, RootMessageSubmission,
};
use chat_subscription_slack::fake::FakeTransport;
use chat_subscription_slack::outbound::{handle_request, OutboundConfig};
use chat_subscription_slack::plugin::PluginConfig;
use chat_subscription_slack::transport::SlackTransport;
use serde_json::{json, Map};

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
fn a_bridge_configuration_for_slack_is_accepted_by_the_plugin() {
    let configuration = BridgeConfiguration {
        subscription_plugin: "slack".to_owned(),
        subscription_environment: Vec::new(),
        channel_ids: vec![CHANNEL.to_owned()],
        allowed_senders: vec!["U0OWNER0001".to_owned()],
        agent_name: "slack-test".to_owned(),
        agent_label: "slack test agent".to_owned(),
        outbound_enabled: true,
        ack_reaction: ReactionSetting::Emoji("🤖".to_owned()),
        receipt_reaction: ReactionSetting::Inherit,
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
    };
    let request = configuration
        .subscribe_request(Some("slack1:seq:42"))
        .expect("the bridge builds the request");
    let parsed = PluginConfig::from_request(&request).expect("the plugin accepts it");
    assert_eq!(parsed.ignore_text_prefixes, ["[slack test agent]"]);
}
