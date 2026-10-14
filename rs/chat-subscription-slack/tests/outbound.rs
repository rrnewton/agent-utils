//! Outbound helper behaviour against the fake transport: thread replies, the ACK reaction,
//! refusal without an app identity, and idempotent recovery of uncertain posts.

use std::path::{Path, PathBuf};

use chat_subscription_slack::fake::{FakePostFailure, FakeTransport};
use chat_subscription_slack::outbound::{handle_request, OutboundConfig};
use chat_subscription_slack::transport::{SlackTransport, TextFormat, TransportError};
use serde_json::{json, Value};

const CHANNEL: &str = "C0C91RW5FAR";
const APP: &str = "A0TESTAPP01";
const ID: &str = "0b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b";
const ID2: &str = "1b7e4f6a-1c2d-4e5f-9a9b-0c1d2e3f4a5b";

fn ledger(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("slack-outbound-ledgers")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn config(ledger: &Path, app: Option<&str>) -> OutboundConfig {
    let mut arguments = vec![
        "--cli=/opt/slack/bin/slack-cli".to_owned(),
        format!("--channel={CHANNEL}"),
        format!("--ledger={}", ledger.display()),
        "--max-chunk-bytes=1000".to_owned(),
    ];
    if let Some(app) = app {
        arguments.push(format!("--app-id={app}"));
    }
    OutboundConfig::parse(&arguments).expect("valid helper arguments")
}

fn run(config: &OutboundConfig, fake: &FakeTransport, request: &Value, now: u64) -> Value {
    let mut transport = || Ok(Box::new(fake.clone()) as Box<dyn SlackTransport>);
    let clock = move || now;
    let mut line = serde_json::to_vec(request).unwrap();
    line.push(b'\n');
    let response = handle_request(&line, Ok(config), &mut transport, &clock);
    assert_eq!(response.last(), Some(&b'\n'), "one response line");
    assert_eq!(response.iter().filter(|byte| **byte == b'\n').count(), 1);
    serde_json::from_slice(&response).expect("response is JSON")
}

fn send(id: &str, thread: Option<&str>, text: &str) -> Value {
    json!({
        "version": 1, "id": id, "action": "send", "channel_id": CHANNEL,
        "thread_id": thread.map(|ts| format!("{CHANNEL}/threads/{ts}")),
        "text": text,
    })
}

#[test]
fn reply_goes_into_the_request_thread_as_the_app() {
    let ledger = ledger("thread");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let response = run(
        &config,
        &fake,
        &send(ID, Some("1792000801.000100"), "**done**"),
        1_000,
    );
    assert_eq!(response["ok"], json!(true), "{response}");
    assert_eq!(response["id"], json!(ID));
    assert_eq!(response["action"], json!("send"));
    let message_id = response["receipt"]["message_id"].as_str().unwrap();
    assert!(message_id.starts_with("C0C91RW5FAR/messages/"));
    let state = fake.state();
    assert_eq!(state.posts.len(), 1);
    assert_eq!(state.posts[0].app_id, APP);
    assert_eq!(
        state.posts[0].thread_ts.as_deref(),
        Some("1792000801.000100")
    );
    assert_eq!(state.posts[0].format, TextFormat::Markdown);
    assert_eq!(
        message_id,
        format!("{CHANNEL}/messages/{}", state.posts[0].ts)
    );
}

#[test]
fn ack_reaction_maps_the_emoji_and_is_idempotent() {
    let ledger = ledger("reaction");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let request = json!({
        "version": 1, "id": ID, "action": "ensure_reaction", "channel_id": CHANNEL,
        "message_id": format!("{CHANNEL}/messages/1792000801.000100"), "emoji": "🤖",
    });
    let first = run(&config, &fake, &request, 1_000);
    assert_eq!(first["ok"], json!(true), "{first}");
    assert_eq!(
        first["receipt"]["reaction_id"],
        json!(format!(
            "{CHANNEL}/messages/1792000801.000100/reactions/robot_face"
        ))
    );
    assert_eq!(first["receipt"]["already_present"], json!(false));
    let second = run(&config, &fake, &request, 2_000);
    assert_eq!(second["receipt"]["already_present"], json!(true));
    let state = fake.state();
    assert_eq!(state.reactions.len(), 1);
    assert_eq!(state.reactions[0].app_id, APP);
    assert_eq!(state.reactions[0].name, "robot_face");
}

#[test]
fn without_an_app_id_nothing_is_posted() {
    let ledger = ledger("no-app");
    let config = config(&ledger, None);
    let fake = FakeTransport::new();
    let mut built = 0;
    let mut transport = || {
        built += 1;
        Ok(Box::new(fake.clone()) as Box<dyn SlackTransport>)
    };
    let line = serde_json::to_vec(&send(ID, Some("1792000801.000100"), "hello")).unwrap();
    let response: Value =
        serde_json::from_slice(&handle_request(&line, Ok(&config), &mut transport, &|| 1)).unwrap();
    assert_eq!(response["ok"], json!(false));
    assert_eq!(response["error"]["code"], json!("app_identity_required"));
    assert_eq!(response["error"]["outcome"], json!("not_applied"));
    assert_eq!(built, 0, "the transport is never built without an app id");
    assert!(fake.state().posts.is_empty());

    // A user ID is not accepted as the posting identity.
    assert!(OutboundConfig::parse(&[
        "--cli=/opt/slack/bin/slack-cli".to_owned(),
        format!("--channel={CHANNEL}"),
        format!("--ledger={}", ledger.display()),
        "--app-id=U0OWNER0001".to_owned(),
    ])
    .is_err());
}

#[test]
fn a_retried_operation_returns_the_recorded_message() {
    let ledger = ledger("retry");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let request = send(ID, Some("1792000801.000100"), "once");
    let first = run(&config, &fake, &request, 1_000);
    let second = run(&config, &fake, &request, 5_000);
    assert_eq!(first["receipt"], second["receipt"]);
    assert_eq!(fake.state().posts.len(), 1);

    // The same UUID for a different message is refused.
    let conflict = run(
        &config,
        &fake,
        &send(ID, Some("1792000801.000100"), "different"),
        6_000,
    );
    assert_eq!(conflict["error"]["code"], json!("operation_conflict"));
}

#[test]
fn a_lost_response_is_reconciled_without_a_second_post() {
    let ledger = ledger("lost-response");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.push_post_failure(FakePostFailure::PostedButLost);
    let request = send(ID, Some("1792000801.000100"), "[agent] the answer is 42");
    let first = run(&config, &fake, &request, 1_792_000_900_000);
    assert_eq!(first["error"]["outcome"], json!("unknown"));
    assert_eq!(first["error"]["retryable"], json!(true));
    let second = run(&config, &fake, &request, 1_792_000_960_000);
    assert_eq!(second["ok"], json!(true), "{second}");
    let state = fake.state();
    assert_eq!(
        state.posts.len(),
        1,
        "the earlier post was adopted, not repeated"
    );
    assert_eq!(
        second["receipt"]["message_id"],
        json!(format!("{CHANNEL}/messages/{}", state.posts[0].ts))
    );
}

#[test]
fn a_lost_request_is_reposted_only_after_the_uncertainty_window() {
    let ledger = ledger("lost-request");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.push_post_failure(FakePostFailure::LostBeforePosting);
    let request = send(ID, Some("1792000801.000100"), "[agent] retry me");
    let start = 1_792_000_900_000;
    assert_eq!(
        run(&config, &fake, &request, start)["error"]["outcome"],
        json!("unknown")
    );
    let early = run(&config, &fake, &request, start + 30_000);
    assert_eq!(early["error"]["code"], json!("outcome_unresolved"));
    assert!(fake.state().posts.is_empty());
    let late = run(&config, &fake, &request, start + 121_000);
    assert_eq!(late["ok"], json!(true), "{late}");
    assert_eq!(fake.state().posts.len(), 1);
}

#[test]
fn slack_refusals_pass_through_as_not_applied() {
    let ledger = ledger("refused");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.push_post_failure(FakePostFailure::Refuse(TransportError::not_applied(
        "not_in_channel",
        "the app is not a member of the channel",
        true,
    )));
    let request = send(ID, Some("1792000801.000100"), "hello");
    let refused = run(&config, &fake, &request, 1_000);
    assert_eq!(refused["error"]["code"], json!("not_in_channel"));
    assert_eq!(refused["error"]["outcome"], json!("not_applied"));
    // After the app is invited, the same UUID posts normally and immediately.
    assert_eq!(run(&config, &fake, &request, 2_000)["ok"], json!(true));
    assert_eq!(fake.state().posts.len(), 1);
}

#[test]
fn local_checks_refuse_before_contacting_slack() {
    let ledger = ledger("local");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let other_channel = json!({
        "version": 1, "id": ID, "action": "send", "channel_id": "C0OTHER0001",
        "thread_id": null, "text": "x",
    });
    assert_eq!(
        run(&config, &fake, &other_channel, 1)["error"]["code"],
        json!("channel_not_allowed")
    );
    let crossed_thread = json!({
        "version": 1, "id": ID, "action": "send", "channel_id": CHANNEL,
        "thread_id": "C0OTHER0001/threads/1792000801.000100", "text": "x",
    });
    assert_eq!(
        run(&config, &fake, &crossed_thread, 1)["error"]["code"],
        json!("request_invalid")
    );
    let unmapped = json!({
        "version": 1, "id": ID2, "action": "ensure_reaction", "channel_id": CHANNEL,
        "message_id": format!("{CHANNEL}/messages/1792000801.000100"), "emoji": "🦀",
    });
    assert_eq!(
        run(&config, &fake, &unmapped, 1)["error"]["code"],
        json!("reaction_unsupported")
    );
    let garbage = handle_request(b"not json\n", Ok(&config), &mut || unreachable!(), &|| 1);
    let garbage: Value = serde_json::from_slice(&garbage).unwrap();
    assert_eq!(garbage["id"], Value::Null);
    assert!(fake.state().posts.is_empty() && fake.state().reactions.is_empty());
}

#[test]
fn long_bodies_are_posted_in_order_and_resume_after_a_partial_failure() {
    let body = (0..30)
        .map(|line| format!("line {line:02} {}\n", "x".repeat(80)))
        .collect::<String>();

    let ledger = ledger("chunks");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let first = run(&config, &fake, &send(ID, None, &body), 1_000);
    assert_eq!(first["ok"], json!(true), "{first}");
    let posts = fake.state().posts.clone();
    assert!(posts.len() >= 3, "a 2.6 KB body is split into 1 KB chunks");
    assert_eq!(
        posts
            .iter()
            .map(|post| post.text.as_str())
            .collect::<String>(),
        body
    );
    assert!(posts
        .iter()
        .all(|post| post.thread_ts.is_none() && post.app_id == APP));
    assert_eq!(
        first["receipt"]["message_id"],
        json!(format!("{CHANNEL}/messages/{}", posts[0].ts))
    );

    // Chunk two is refused; the retry of the same UUID posts only the remaining chunks.
    let ledger = self::ledger("chunks-partial");
    let config = self::config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let line = serde_json::to_vec(&send(ID2, None, &body)).unwrap();
    let mut refusing = Some(SecondRefusal {
        inner: fake.clone(),
        posts: 0,
    });
    let mut wrapped =
        || Ok(Box::new(refusing.take().expect("built once")) as Box<dyn SlackTransport>);
    let response: Value =
        serde_json::from_slice(&handle_request(&line, Ok(&config), &mut wrapped, &|| 1_000))
            .unwrap();
    assert_eq!(response["error"]["code"], json!("ratelimited"));
    assert_eq!(fake.state().posts.len(), 1);
    let mut plain = || Ok(Box::new(fake.clone()) as Box<dyn SlackTransport>);
    let response: Value =
        serde_json::from_slice(&handle_request(&line, Ok(&config), &mut plain, &|| 2_000)).unwrap();
    assert_eq!(response["ok"], json!(true), "{response}");
    let posts = fake.state().posts.clone();
    assert_eq!(
        posts
            .iter()
            .map(|post| post.text.as_str())
            .collect::<String>(),
        body
    );
}

/// A transport that lets one post through and refuses the next with `ratelimited`.
struct SecondRefusal {
    inner: FakeTransport,
    posts: usize,
}

impl SlackTransport for SecondRefusal {
    fn open_stream(
        &mut self,
        spec: &chat_subscription_slack::transport::SubscribeSpec,
    ) -> Result<Box<dyn chat_subscription_slack::transport::EventStream>, TransportError> {
        self.inner.open_stream(spec)
    }

    fn post_message(
        &mut self,
        post: &chat_subscription_slack::transport::PostMessage<'_>,
    ) -> Result<String, TransportError> {
        self.posts += 1;
        if self.posts == 2 {
            return Err(TransportError::not_applied(
                "ratelimited",
                "slow down",
                true,
            ));
        }
        self.inner.post_message(post)
    }

    fn add_reaction(
        &mut self,
        app_id: &chat_subscription_slack::ids::AppId,
        channel_id: &str,
        message_ts: &str,
        name: &str,
    ) -> Result<bool, TransportError> {
        self.inner
            .add_reaction(app_id, channel_id, message_ts, name)
    }

    fn list_messages(
        &mut self,
        query: &chat_subscription_slack::transport::ListQuery<'_>,
    ) -> Result<Vec<chat_subscription_slack::transport::ListedMessage>, TransportError> {
        self.inner.list_messages(query)
    }
}

/// A transport whose listing always fails, as for an app without history access.
struct FailingListing {
    inner: FakeTransport,
}

impl SlackTransport for FailingListing {
    fn open_stream(
        &mut self,
        spec: &chat_subscription_slack::transport::SubscribeSpec,
    ) -> Result<Box<dyn chat_subscription_slack::transport::EventStream>, TransportError> {
        self.inner.open_stream(spec)
    }

    fn post_message(
        &mut self,
        post: &chat_subscription_slack::transport::PostMessage<'_>,
    ) -> Result<String, TransportError> {
        self.inner.post_message(post)
    }

    fn add_reaction(
        &mut self,
        app_id: &chat_subscription_slack::ids::AppId,
        channel_id: &str,
        message_ts: &str,
        name: &str,
    ) -> Result<bool, TransportError> {
        self.inner
            .add_reaction(app_id, channel_id, message_ts, name)
    }

    fn list_messages(
        &mut self,
        _query: &chat_subscription_slack::transport::ListQuery<'_>,
    ) -> Result<Vec<chat_subscription_slack::transport::ListedMessage>, TransportError> {
        Err(TransportError::not_applied(
            "missing_scope",
            "no history scope",
            true,
        ))
    }
}

#[test]
fn a_failing_listing_still_reposts_after_the_window() {
    let ledger = ledger("failing-listing");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.push_post_failure(FakePostFailure::LostBeforePosting);
    let line = serde_json::to_vec(&send(ID, Some("1792000801.000100"), "[agent] hi")).unwrap();
    let mut transport = || {
        Ok(Box::new(FailingListing {
            inner: fake.clone(),
        }) as Box<dyn SlackTransport>)
    };
    let start = 1_792_000_900_000;
    let mut at = |now: u64| -> Value {
        serde_json::from_slice(&handle_request(&line, Ok(&config), &mut transport, &|| now))
            .unwrap()
    };
    assert_eq!(at(start)["error"]["outcome"], json!("unknown"));
    assert_eq!(
        at(start + 60_000)["error"]["code"],
        json!("outcome_unresolved")
    );
    assert_eq!(at(start + 121_000)["ok"], json!(true));
    assert_eq!(fake.state().posts.len(), 1);
}
