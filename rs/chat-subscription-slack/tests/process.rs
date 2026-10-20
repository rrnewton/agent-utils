//! The real executables, driven the way the bridge drives them, against a fake Slack client
//! script. This checks the client command lines (`--as=<app>` on every write of a client that
//! holds a person's credentials, and none for a Crewmate) and the process protocol framing end to
//! end.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use chat_subscription::{
    BackendConfiguration, ChannelId, ChatSubscription, CommittableEvent, ProviderCursor, SenderId,
    SubscribeRequest, SubscriptionItem,
};
use chat_subscription_plugin::process::{ProcessPhaseTimeouts, ProcessPluginChild};
use serde_json::{json, Map, Value};

const CHANNEL: &str = "C0C91RW5FAR";
const OWNER: &str = "U0OWNER0001";
const APP: &str = "A0TESTAPP01";

const FAKE_CLIENT: &str = r#"#!/bin/sh
# Fake Slack client: records each invocation's arguments, then answers like the real one.
printf '%s\n' "$*" >> "$FAKE_SLACK_DIR/argv.log"
# Global flags, such as one that selects a Crewmate, come before the subcommand.
while [ "${1#--}" != "$1" ]; do shift; done
case "$1 $2" in
  "slack.conversation tail")
    cat "$FAKE_SLACK_DIR/tail.jsonl"
    exec sleep 30 ;;
  "slack.message send")
    cat > "$FAKE_SLACK_DIR/body.txt"
    printf '%s\n' '{"data":{"channel_id":"C0C91RW5FAR","message_ts":"1792000999.000200"}}' ;;
  "slack.reaction add")
    printf '%s\n' '{"data":{"changed":true}}' ;;
  *)
    printf '%s\n' '{"status":"error","message":"unexpected command"}'
    exit 1 ;;
esac
"#;

fn workspace(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("slack-process")
        .join(name);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let client = root.join("slack-cli");
    fs::write(&client, FAKE_CLIENT).unwrap();
    fs::set_permissions(&client, fs::Permissions::from_mode(0o700)).unwrap();
    root
}

fn invocations(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("argv.log"))
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn timeouts() -> ProcessPhaseTimeouts {
    let five = Duration::from_secs(5);
    ProcessPhaseTimeouts::new(five, five, five, five, Duration::from_secs(1)).unwrap()
}

#[test]
fn plugin_process_delivers_a_slack_message() {
    let root = workspace("plugin");
    let line = json!({
        "seq": 1, "ts": 1.0, "type": "message.created", "source": "slack", "iris_seq_id": 501,
        "payload": {"event_type": "message.created", "container_id": CHANNEL,
            "slack_message": {"channel_id": CHANNEL, "message_ts": "4102444800.000100",
                "slack_user_id": OWNER, "text": "hello bridge"}}
    });
    fs::write(root.join("tail.jsonl"), format!("{line}\n")).unwrap();
    let data = Map::from_iter([
        ("cli_path".to_owned(), json!(root.join("slack-cli"))),
        (
            "cli_environment".to_owned(),
            json!({"FAKE_SLACK_DIR": root}),
        ),
        ("diagnostics_log".to_owned(), json!(root.join("plugin.log"))),
    ]);
    let request = SubscribeRequest::new(
        vec![ChannelId::new(CHANNEL).unwrap()],
        vec![SenderId::new(OWNER).unwrap()],
        None::<ProviderCursor>,
    )
    .unwrap()
    .with_backend_configuration(BackendConfiguration::new("agentctl.slack.v1", data).unwrap());

    let child =
        ProcessPluginChild::spawn(Command::new(env!("CARGO_BIN_EXE_agentctl-slack-plugin")))
            .expect("plugin starts");
    let (mut backend, cancellation) = child.connect(timeouts()).expect("handshake");
    {
        let mut subscription = ChatSubscription::open(&mut backend, &request).expect("subscribed");
        let mut batches = Vec::new();
        while batches.len() < 2 {
            match subscription.next_item().expect("item") {
                Some(SubscriptionItem::Batch(batch)) => {
                    subscription.commit_durable(&batch).expect("commit");
                    batches.push(batch);
                }
                Some(SubscriptionItem::Heartbeat(_)) => {}
                None => panic!("plugin ended early"),
            }
        }
        assert_eq!(batches[0].events(), [CommittableEvent::Checkpoint]);
        assert!(batches[0].cursor().as_str().starts_with("slack1:since:"));
        assert_eq!(batches[1].cursor().as_str(), "slack1:seq:501");
        match batches[1].events() {
            [CommittableEvent::MessageCreated(message)] => {
                assert_eq!(message.text(), "hello bridge");
                assert_eq!(
                    message.thread_id().as_str(),
                    "C0C91RW5FAR/threads/4102444800.000100"
                );
            }
            other => panic!("expected a message, found {other:?}"),
        }
    }
    drop(backend);
    let status = cancellation.cancel().expect("plugin reaped");
    assert!(status.success(), "plugin exited with {status}");
    let calls = invocations(&root);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].starts_with("slack.conversation tail --channel-id=C0C91RW5FAR"));
    assert!(calls[0].contains("--event-types=message.created"));
    assert!(calls[0].contains("--include-iris-seq-id"));
    assert!(calls[0].contains("--catchup=none"));
}

fn run_helper(root: &Path, arguments: &[String], request: &Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentctl-slack-outbound"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("FAKE_SLACK_DIR", root)
        .spawn()
        .expect("helper starts");
    let mut line = serde_json::to_vec(request).unwrap();
    line.push(b'\n');
    child.stdin.take().unwrap().write_all(&line).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        text.matches('\n').count(),
        1,
        "exactly one response line: {text:?}"
    );
    serde_json::from_str(&text).unwrap()
}

fn helper_arguments(root: &Path, app: Option<&str>) -> Vec<String> {
    let mut arguments = vec![
        format!("--cli={}", root.join("slack-cli").display()),
        format!("--cli-env=FAKE_SLACK_DIR={}", root.display()),
        format!("--channel={CHANNEL}"),
        format!("--ledger={}", root.join("ledger").display()),
        format!("--diagnostics-log={}", root.join("helper.log").display()),
    ];
    if let Some(app) = app {
        arguments.push(format!("--app-id={app}"));
    }
    arguments
}

#[test]
fn helper_process_posts_in_thread_and_reacts_as_the_app() {
    let root = workspace("helper");
    let arguments = helper_arguments(&root, Some(APP));
    let send = json!({
        "version": 1, "id": "0b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b", "action": "send",
        "channel_id": CHANNEL, "thread_id": format!("{CHANNEL}/threads/1792000801.000100"),
        "text": "**done** -- see the PR",
    });
    let response = run_helper(&root, &arguments, &send);
    assert_eq!(
        response,
        json!({"version": 1, "id": "0b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b", "action": "send",
            "ok": true, "receipt": {"message_id": "C0C91RW5FAR/messages/1792000999.000200"}})
    );
    assert_eq!(
        fs::read_to_string(root.join("body.txt")).unwrap(),
        "**done** -- see the PR"
    );

    let reaction = json!({
        "version": 1, "id": "1b7e4f6a-1c2d-4e5f-9a9b-0c1d2e3f4a5b", "action": "ensure_reaction",
        "channel_id": CHANNEL, "message_id": format!("{CHANNEL}/messages/1792000801.000100"),
        "emoji": "🤖",
    });
    let response = run_helper(&root, &arguments, &reaction);
    assert_eq!(response["receipt"]["already_present"], json!(false));

    let calls = invocations(&root);
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(
        calls[0],
        "slack.message send --channel-id=C0C91RW5FAR --markdown=file://- \
         --thread-ts=1792000801.000100 --as=A0TESTAPP01 --output=json"
    );
    assert_eq!(
        calls[1],
        "slack.reaction add --channel-id=C0C91RW5FAR --message-ts=1792000801.000100 \
         --name=robot_face --as=A0TESTAPP01 --output=json"
    );
    for call in &calls {
        assert!(call.contains("--as=A0TESTAPP01") && !call.contains("--as-user"));
    }
}

#[test]
fn helper_process_as_a_crewmate_never_passes_as() {
    // #383 slack-crewmate-outbound: a client run as the app's Crewmate refuses `--as`, and it
    // needs its own selecting flag and certificate environment, which the operator wrapped in a
    // script. The helper now does both itself.
    let root = workspace("helper-crewmate");
    let mut arguments = helper_arguments(&root, Some(APP));
    arguments.extend([
        "--client-identity=crewmate".to_owned(),
        "--cli-arg=--act-as-bot=1234".to_owned(),
        "--cli-env=CLIENT_CERT_PATH=/etc/certs/user.pem".to_owned(),
    ]);
    let send = json!({
        "version": 1, "id": "0b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b", "action": "send",
        "channel_id": CHANNEL, "thread_id": format!("{CHANNEL}/threads/1792000801.000100"),
        "text": "done",
    });
    let response = run_helper(&root, &arguments, &send);
    assert_eq!(response["ok"], json!(true), "{response}");
    let reaction = json!({
        "version": 1, "id": "1b7e4f6a-1c2d-4e5f-9a9b-0c1d2e3f4a5b", "action": "ensure_reaction",
        "channel_id": CHANNEL, "message_id": format!("{CHANNEL}/messages/1792000801.000100"),
        "emoji": "👀",
    });
    let response = run_helper(&root, &arguments, &reaction);
    assert_eq!(response["ok"], json!(true), "{response}");
    let calls = invocations(&root);
    assert_eq!(
        calls,
        [
            "--act-as-bot=1234 slack.message send --channel-id=C0C91RW5FAR \
             --markdown=file://- --thread-ts=1792000801.000100 --output=json",
            "--act-as-bot=1234 slack.reaction add --channel-id=C0C91RW5FAR \
             --message-ts=1792000801.000100 --name=eyes --output=json",
        ]
    );
}

#[test]
fn helper_process_without_an_app_id_never_runs_the_client() {
    let root = workspace("helper-no-app");
    let arguments = helper_arguments(&root, None);
    let send = json!({
        "version": 1, "id": "0b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b", "action": "send",
        "channel_id": CHANNEL, "thread_id": null, "text": "hello",
    });
    let response = run_helper(&root, &arguments, &send);
    assert_eq!(response["ok"], json!(false));
    assert_eq!(response["error"]["code"], json!("app_identity_required"));
    assert_eq!(response["error"]["outcome"], json!("not_applied"));
    assert!(invocations(&root).is_empty());
}
