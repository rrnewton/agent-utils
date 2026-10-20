//! Directory reads and guarded rejection reactions through the real helper and a fake client.

use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use agentctl::chat_runtime::{BridgeConfiguration, BridgeState};
use serde_json::{json, Value};

const CHANNEL: &str = "C0C91RW5FAR";
const APP: &str = "A0TESTAPP01";
const ID: &str = "0b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b";
const MEMBER1: &str = "U0MEMBER001";
const MEMBER2: &str = "W0MEMBER002";

const FAKE_CLIENT: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_SLACK_DIR/argv.log"
while [ "${1#--}" != "$1" ]; do shift; done
case "$1 $2" in
  "slack.user describe")
    if [ -f "$FAKE_SLACK_DIR/author-slow" ]; then sleep 0.65; fi
    if [ -f "$FAKE_SLACK_DIR/author-timeout" ]; then sleep 1.2; fi
    cat "$FAKE_SLACK_DIR/resolve.json" ;;
  "slack.reaction add")
    if [ -f "$FAKE_SLACK_DIR/reaction-slow" ]; then sleep 0.65; fi
    printf '%s\n' "$*" >> "$FAKE_SLACK_DIR/reactions.log"
    printf '%s\n' '{"status":"success","data":{"changed":true}}' ;;
  "slack.conversation members")
    page=members-first.json
    for arg in "$@"; do
      case "$arg" in
        --page-token=next-page) page=members-next.json ;;
        --page-token=*) exit 1 ;;
      esac
    done
    if [ -f "$FAKE_SLACK_DIR/slow" ]; then sleep 0.65; fi
    cat "$FAKE_SLACK_DIR/$page" ;;
  *)
    printf '%s\n' '{"status":"error","message":"unexpected command"}'
    exit 1 ;;
esac
"#;

fn workspace(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("slack-directory")
        .join(name);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("slack-cli"), FAKE_CLIENT).unwrap();
    fs::set_permissions(root.join("slack-cli"), fs::Permissions::from_mode(0o700)).unwrap();
    root
}

fn page(ids: &[&str], next: Option<&str>) -> Value {
    json!({
        "data": ids.iter().map(|id| json!({"user_id": id})).collect::<Vec<_>>(),
        "pagination": {"has_next_page": next.is_some(), "next_page_token": next.unwrap_or(""), "page_size": ids.len()},
        "completeness": {"source_complete": true},
        "dropped_row_count": 0,
    })
}

fn fixture(root: &Path, name: &str, value: &Value) {
    fs::write(root.join(name), serde_json::to_vec(value).unwrap()).unwrap();
}

fn arguments(root: &Path) -> Vec<String> {
    vec![
        format!("--cli={}", root.join("slack-cli").display()),
        format!("--cli-env=FAKE_SLACK_DIR={}", root.display()),
        format!("--channel={CHANNEL}"),
        format!("--app-id={APP}"),
        format!("--ledger={}", root.join("ledger").display()),
        "--timeout-ms=1000".to_owned(),
    ]
}

fn run(root: &Path, arguments: &[String], request: &Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentctl-slack-outbound"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("FAKE_SLACK_DIR", root)
        .spawn()
        .unwrap();
    let mut input = serde_json::to_vec(request).unwrap();
    input.push(b'\n');
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        output.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["version"], json!(1));
    assert_eq!(response["id"], request["id"]);
    assert_eq!(response["action"], request["action"]);
    response
}

fn calls(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("argv.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn listing_request() -> Value {
    json!({"version": 1, "id": ID, "action": "list_members", "channel_id": CHANNEL})
}

fn rejection_request(sender: &str) -> Value {
    json!({"version": 1, "id": ID, "action": "ensure_rejection", "channel_id": CHANNEL,
        "message_id": format!("{CHANNEL}/messages/1792000801.000100"),
        "sender_id": sender, "emoji": "❌"})
}

fn rejection_journal(root: &Path) -> PathBuf {
    root.join("ledger/rejections").join(format!("{ID}.json"))
}

fn assert_no_post_intent(root: &Path) {
    assert!(
        !root.join("ledger").join(format!("{ID}.json")).exists(),
        "rejections never create posting intents"
    );
}

fn assert_refused(response: &Value, code: &str) {
    assert_eq!(response["ok"], json!(false), "{response}");
    assert_eq!(response["error"]["code"], json!(code), "{response}");
    assert_eq!(response["error"]["outcome"], json!("not_applied"));
    assert!(
        response.get("receipt").is_none(),
        "no partial success: {response}"
    );
}

#[test]
fn lookup_uses_the_cli_user_selector_and_returns_only_the_provider_id() {
    let root = workspace("resolve");
    fixture(
        &root,
        "resolve.json",
        &json!({"data": {"user_id": MEMBER1, "name": "member"}}),
    );
    let request = json!({"version": 1, "id": ID, "action": "resolve_sender", "channel_id": CHANNEL, "query": "member@example.invalid"});
    let response = run(&root, &arguments(&root), &request);
    assert_eq!(response["ok"], json!(true), "{response}");
    assert_eq!(response["receipt"], json!({"sender_id": MEMBER1}));
    assert_eq!(calls(&root), [format!("slack.user describe --user=member@example.invalid --columns=user_id --as={APP} --output=json")]);
    assert!(
        !root.join("ledger").exists(),
        "reads do not create operation intents"
    );
}

#[test]
fn rejection_looks_up_the_exact_human_account_before_writing_as_the_app() {
    let root = workspace("human-rejection");
    fixture(
        &root,
        "resolve.json",
        &json!({"data": {"user_id": MEMBER1, "is_bot": false}}),
    );
    let mut args = arguments(&root);
    args.push(format!(
        "--warm-socket=/tmp/slack-rejection-unused-{}.sock",
        std::process::id()
    ));
    let response = run(&root, &args, &rejection_request(MEMBER1));
    assert_eq!(response["ok"], json!(true), "{response}");
    assert_eq!(
        response["receipt"],
        json!({
            "reaction_id": format!("{CHANNEL}/messages/1792000801.000100/reactions/x"),
            "already_present": false,
        })
    );
    assert_eq!(calls(&root), [
        format!("slack.user describe --user={MEMBER1} --columns=user_id,is_bot --as={APP} --output=json"),
        format!("slack.reaction add --channel-id={CHANNEL} --message-ts=1792000801.000100 --name=x --as={APP} --output=json"),
    ]);
    assert_eq!(
        fs::read_to_string(root.join("reactions.log"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_no_post_intent(&root);
    let record: Value =
        serde_json::from_slice(&fs::read(rejection_journal(&root)).unwrap()).unwrap();
    assert_eq!(record["name"], "x");
    assert_eq!(record["complete"], true);
}

#[test]
fn rejection_never_writes_for_bot_unknown_ambiguous_or_mismatched_accounts() {
    for (name, account) in [
        (
            "bot",
            json!({"data": {"user_id": "U0BRIDGEBOT", "is_bot": true}}),
        ),
        ("unknown", json!({"data": {"user_id": "U0BRIDGEBOT"}})),
        (
            "null",
            json!({"data": {"user_id": "U0BRIDGEBOT", "is_bot": null}}),
        ),
        (
            "string",
            json!({"data": {"user_id": "U0BRIDGEBOT", "is_bot": "false"}}),
        ),
        (
            "mismatch",
            json!({"data": {"user_id": MEMBER1, "is_bot": false}}),
        ),
        (
            "ambiguous",
            json!({"data": [{"user_id": "U0BRIDGEBOT", "is_bot": false}, {"user_id": MEMBER1, "is_bot": false}]}),
        ),
        ("empty", json!({"data": []})),
    ] {
        let root = workspace(&format!("rejection-{name}"));
        fixture(&root, "resolve.json", &account);
        // There are no message bot/app annotations in this action. In particular the bridge's
        // own U-prefixed bot account must be denied by the authoritative lookup alone.
        let response = run(&root, &arguments(&root), &rejection_request("U0BRIDGEBOT"));
        assert_refused(&response, "author_not_human");
        assert_eq!(response["error"]["retryable"], json!(false));
        assert_eq!(calls(&root), [format!("slack.user describe --user=U0BRIDGEBOT --columns=user_id,is_bot --as={APP} --output=json")]);
        assert!(
            !root.join("reactions.log").exists(),
            "{name}: no Slack write"
        );
        assert_no_post_intent(&root);
        assert!(
            !rejection_journal(&root).exists(),
            "{name}: a fresh nonhuman refusal has no possible-effect intent"
        );
    }
}

#[test]
fn rejection_lookup_errors_or_timeouts_never_start_a_write() {
    let root = workspace("rejection-lookup-error");
    fixture(
        &root,
        "resolve.json",
        &json!({"status": "error", "message": "missing_scope"}),
    );
    assert_refused(
        &run(&root, &arguments(&root), &rejection_request(MEMBER1)),
        "missing_scope",
    );
    assert_eq!(calls(&root).len(), 1);
    assert!(!root.join("reactions.log").exists());
    assert_no_post_intent(&root);
    assert!(!rejection_journal(&root).exists());

    let root = workspace("rejection-lookup-timeout");
    fixture(
        &root,
        "resolve.json",
        &json!({"data": {"user_id": MEMBER1, "is_bot": false}}),
    );
    fs::write(root.join("author-timeout"), "").unwrap();
    assert_refused(
        &run(&root, &arguments(&root), &rejection_request(MEMBER1)),
        "transport_timeout",
    );
    assert_eq!(calls(&root).len(), 1);
    assert!(!root.join("reactions.log").exists());
    assert_no_post_intent(&root);
    assert!(!rejection_journal(&root).exists());
}

#[test]
fn rejection_lookup_and_write_share_one_deadline_even_through_the_warm_wrapper() {
    let root = workspace("rejection-shared-deadline");
    fixture(
        &root,
        "resolve.json",
        &json!({"data": {"user_id": MEMBER1, "is_bot": false}}),
    );
    fs::write(root.join("author-slow"), "").unwrap();
    fs::write(root.join("reaction-slow"), "").unwrap();
    let mut args = arguments(&root);
    args.push(format!(
        "--warm-socket=/tmp/slack-rejection-deadline-unused-{}.sock",
        std::process::id()
    ));
    let response = run(&root, &args, &rejection_request(MEMBER1));
    assert_eq!(response["ok"], json!(false), "{response}");
    assert_eq!(response["error"]["code"], json!("transport_timeout"));
    // A write began, so the client conservatively reports an unknown outcome.
    assert_eq!(response["error"]["outcome"], json!("unknown"));
    assert_eq!(calls(&root).len(), 2);
    assert!(
        !root.join("reactions.log").exists(),
        "the shared deadline killed the slow write"
    );
    assert_no_post_intent(&root);
    let record: Value =
        serde_json::from_slice(&fs::read(rejection_journal(&root)).unwrap()).unwrap();
    assert_eq!(record["name"], "x");
    assert_eq!(
        record["complete"], false,
        "an uncertain write retains its first intent"
    );
}

#[test]
fn a_new_helper_replays_the_original_rejection_without_another_cli_call() {
    let root = workspace("rejection-completed-replay");
    fixture(
        &root,
        "resolve.json",
        &json!({"data": {"user_id": MEMBER1, "is_bot": false}}),
    );
    let mut request = rejection_request(MEMBER1);
    let first = run(&root, &arguments(&root), &request);
    assert_eq!(first["ok"], true, "{first}");
    let original = fs::read(rejection_journal(&root)).unwrap();
    let original_calls = calls(&root);
    assert_eq!(original_calls.len(), 2);
    // Historical receipts authorize no new write, even when a fresh account check would refuse.
    fixture(
        &root,
        "resolve.json",
        &json!({"data": {"user_id": MEMBER1, "is_bot": true}}),
    );
    for emoji in ["no_entry_sign", "🚫"] {
        request["emoji"] = json!(emoji);
        let replay = run(&root, &arguments(&root), &request);
        assert_eq!(replay["ok"], true, "{replay}");
        assert_eq!(
            replay["receipt"]["reaction_id"],
            first["receipt"]["reaction_id"]
        );
        assert_eq!(replay["receipt"]["already_present"], true);
        assert_eq!(calls(&root), original_calls);
        assert_eq!(fs::read(rejection_journal(&root)).unwrap(), original);
    }
    assert_eq!(
        fs::read_to_string(root.join("reactions.log"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_no_post_intent(&root);
}

#[test]
fn a_locked_rejection_operation_refuses_without_waiting_or_starting_the_cli() {
    let root = workspace("rejection-busy-operation");
    fixture(
        &root,
        "resolve.json",
        &json!({"data": {"user_id": MEMBER1, "is_bot": false}}),
    );
    let mut request = rejection_request(MEMBER1);
    let first = run(&root, &arguments(&root), &request);
    assert_eq!(first["ok"], true, "{first}");
    let original_calls = calls(&root);
    let original = fs::read(rejection_journal(&root)).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("ledger/rejections").join(format!("{ID}.lock")))
        .unwrap();
    // SAFETY: lock owns a live descriptor; a distinct helper process must see this held lock.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    request["emoji"] = json!("no_entry_sign");
    let child_root = root.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let child = std::thread::spawn(move || {
        let response = run(&child_root, &arguments(&child_root), &request);
        let _ = sender.send(response);
    });
    let before_unlock = receiver.recv_timeout(Duration::from_secs(2));
    drop(lock);
    child.join().unwrap();
    let refused =
        before_unlock.expect("optional rejection feedback must not wait on a held operation lock");
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "rejection_ledger_busy");
    assert_eq!(refused["error"]["outcome"], "unknown");
    assert_eq!(refused["error"]["retryable"], true);
    assert_eq!(calls(&root), original_calls);
    assert_eq!(fs::read(rejection_journal(&root)).unwrap(), original);
    assert_eq!(
        fs::read_to_string(root.join("reactions.log"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn rejection_persistence_time_is_part_of_the_same_cli_deadline() {
    use chat_subscription_slack::outbound::OutboundConfig;
    use chat_subscription_slack::transport::Outcome;

    let root = workspace("rejection-persistence-deadline");
    fixture(
        &root,
        "resolve.json",
        &json!({"data": {"user_id": MEMBER1, "is_bot": false}}),
    );
    fs::write(root.join("author-slow"), "").unwrap();
    let config = OutboundConfig::parse(&arguments(&root)).unwrap();
    let mut transport = config.transport().unwrap();
    let mut persisted = false;
    let error = transport
        .add_rejection(
            config.app_id.as_ref().unwrap(),
            CHANNEL,
            "1792000801.000100",
            MEMBER1,
            "x",
            &mut || {
                persisted = true;
                std::thread::sleep(Duration::from_millis(650));
                Ok(())
            },
        )
        .expect_err("the account lookup and persistence exhausted the 1000ms total deadline");
    assert!(persisted);
    assert_eq!(error.code, "transport_timeout");
    assert_eq!(error.outcome, Outcome::NotApplied);
    assert_eq!(
        calls(&root).len(),
        1,
        "no reaction command after the shared deadline"
    );
    assert!(!root.join("reactions.log").exists());
}

#[test]
fn member_listing_drains_pages_and_returns_sorted_explicit_ids_through_the_warm_wrapper() {
    let root = workspace("pages");
    fixture(
        &root,
        "members-first.json",
        &page(&[MEMBER2], Some("next-page")),
    );
    fixture(&root, "members-next.json", &page(&[MEMBER1, MEMBER2], None));
    let mut args = arguments(&root);
    args.push(format!(
        "--warm-socket=/tmp/slack-directory-unused-{}.sock",
        std::process::id()
    ));
    let response = run(&root, &args, &listing_request());
    assert_eq!(response["ok"], json!(true), "{response}");
    assert_eq!(
        response["receipt"],
        json!({"sender_ids": [MEMBER1, MEMBER2]})
    );
    assert_eq!(calls(&root), [
        format!("slack.conversation members --channel-id={CHANNEL} --limit=20 --columns=user_id --as={APP} --output=json"),
        format!("slack.conversation members --channel-id={CHANNEL} --limit=20 --columns=user_id --page-token=next-page --as={APP} --output=json"),
    ]);
    assert!(!root.join("ledger").exists());
}

#[test]
fn crewmate_directory_reads_keep_prefix_arguments_and_do_not_pass_as() {
    let root = workspace("crewmate");
    fixture(&root, "members-first.json", &page(&[MEMBER1], None));
    let mut args = arguments(&root);
    args.extend([
        "--client-identity=crewmate".to_owned(),
        "--cli-arg=--client-role=bot".to_owned(),
    ]);
    let response = run(&root, &args, &listing_request());
    assert_eq!(response["ok"], json!(true), "{response}");
    assert_eq!(calls(&root), [format!("--client-role=bot slack.conversation members --channel-id={CHANNEL} --limit=20 --columns=user_id --output=json")]);
}

#[test]
fn unauthorized_channel_and_invalid_query_never_start_the_client() {
    let root = workspace("local-refusal");
    let mut listing = listing_request();
    listing["channel_id"] = json!("C0OTHER0001");
    assert_refused(
        &run(&root, &arguments(&root), &listing),
        "channel_not_allowed",
    );
    let lookup = json!({"version": 1, "id": ID, "action": "resolve_sender", "channel_id": CHANNEL, "query": "member\n--as=another"});
    assert_refused(&run(&root, &arguments(&root), &lookup), "request_invalid");
    let mut args = arguments(&root);
    args.retain(|arg| !arg.starts_with("--app-id="));
    assert_refused(
        &run(&root, &args, &listing_request()),
        "app_identity_required",
    );
    assert!(calls(&root).is_empty());
}

#[test]
fn member_listing_refuses_incomplete_dropped_malformed_or_unpaged_results() {
    let mut incomplete = page(&[MEMBER1], None);
    incomplete["completeness"]["source_complete"] = json!(false);
    let mut dropped = page(&[MEMBER1], None);
    dropped["dropped_row_count"] = json!(1);
    let mut unpaged = page(&[MEMBER1], None);
    unpaged.as_object_mut().unwrap().remove("pagination");
    let mut missing_token = page(&[MEMBER1], None);
    missing_token["pagination"]["has_next_page"] = json!(true);
    for (name, value) in [
        ("incomplete", incomplete),
        ("dropped", dropped),
        ("unpaged", unpaged),
        ("missing-token", missing_token),
        ("wildcard", page(&["*"], None)),
    ] {
        let root = workspace(name);
        fixture(&root, "members-first.json", &value);
        assert_refused(
            &run(&root, &arguments(&root), &listing_request()),
            "directory_unreadable",
        );
        assert_eq!(calls(&root).len(), 1);
    }
}

#[test]
fn member_listing_refuses_a_repeated_page_token_without_returning_the_first_page() {
    let root = workspace("repeated-token");
    fixture(
        &root,
        "members-first.json",
        &page(&[MEMBER1], Some("next-page")),
    );
    fixture(
        &root,
        "members-next.json",
        &page(&[MEMBER2], Some("next-page")),
    );
    assert_refused(
        &run(&root, &arguments(&root), &listing_request()),
        "directory_unreadable",
    );
    assert_eq!(calls(&root).len(), 2);
}

#[test]
fn member_listing_refuses_more_than_the_bridge_limit_without_truncating() {
    let root = workspace("member-limit");
    let ids = (0..=chat_subscription::MAX_ALLOWED_SENDERS)
        .map(|n| format!("U{n:010}"))
        .collect::<Vec<_>>();
    let borrowed = ids.iter().map(String::as_str).collect::<Vec<_>>();
    let limit = chat_subscription::MAX_ALLOWED_SENDERS;
    fixture(
        &root,
        "members-first.json",
        &page(&borrowed[..limit], Some("next-page")),
    );
    fixture(&root, "members-next.json", &page(&borrowed[limit..], None));
    assert_refused(
        &run(&root, &arguments(&root), &listing_request()),
        "member_limit_exceeded",
    );
    assert_eq!(calls(&root).len(), 2);
}

#[test]
fn member_listing_has_one_deadline_for_all_pages() {
    let root = workspace("deadline");
    fixture(
        &root,
        "members-first.json",
        &page(&[MEMBER1], Some("next-page")),
    );
    fixture(&root, "members-next.json", &page(&[MEMBER2], None));
    fs::write(root.join("slow"), "").unwrap();
    assert_refused(
        &run(&root, &arguments(&root), &listing_request()),
        "transport_timeout",
    );
    assert_eq!(calls(&root).len(), 2);
}

#[test]
fn lookup_refuses_ambiguous_or_invalid_identity_results() {
    for (name, value) in [
        (
            "ambiguous",
            json!({"data": [{"user_id": MEMBER1}, {"user_id": MEMBER2}]}),
        ),
        ("invalid-id", json!({"data": {"user_id": "*"}})),
    ] {
        let root = workspace(name);
        fixture(&root, "resolve.json", &value);
        let request = json!({"version": 1, "id": ID, "action": "resolve_sender", "channel_id": CHANNEL, "query": "member"});
        assert_refused(
            &run(&root, &arguments(&root), &request),
            "directory_unreadable",
        );
    }
}

#[test]
fn directory_output_must_be_one_complete_document() {
    let root = workspace("extra-document");
    fs::write(
        root.join("members-first.json"),
        format!("{}\n{}\n", page(&[MEMBER1], None), page(&[MEMBER2], None)),
    )
    .unwrap();
    assert_refused(
        &run(&root, &arguments(&root), &listing_request()),
        "directory_unreadable",
    );
}

#[test]
fn bridge_grants_consume_the_native_slack_directory_protocol_and_keep_explicit_provenance() {
    let root = workspace("bridge-directory");
    let state_root = root.join("bridge");
    let owner = "U0OWNER0001";
    let later_member = "U0LATER0001";
    let resolved_member = "U0RESOLVE01";
    fixture(
        &root,
        "members-first.json",
        &page(&[owner, MEMBER2], Some("next-page")),
    );
    fixture(&root, "members-next.json", &page(&[MEMBER1, MEMBER2], None));
    let helper = root.join("agentctl-slack-outbound");
    fs::copy(env!("CARGO_BIN_EXE_agentctl-slack-outbound"), &helper)
        .expect("copy the Cargo binary to a private, single-linked helper");
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o500))
        .expect("make the private helper executable only by its owner");
    let configuration: BridgeConfiguration = serde_json::from_value(json!({
        "subscription_plugin": "slack",
        "channel_ids": [CHANNEL],
        "allowed_senders": [owner],
        "agent_name": "coordinator",
        "agent_label": "coordinator",
        "outbound_enabled": true,
        "ack_reaction": null,
        "outbound_command": {
            "executable": fs::canonicalize(helper).unwrap(),
            "arguments": arguments(&root),
            "environment": [],
            "timeout_millis": 30_000,
        },
    }))
    .expect("bridge configuration");
    BridgeState::initialize(&state_root, configuration).expect("private bridge state");

    let result = agentctl::chat_service::grant(
        &state_root,
        &[],
        true,
        Duration::from_secs(43_200),
        Some("current channel discussion"),
    )
    .expect("grant each current member through the native helper");
    assert_eq!(result["permanent_owners"], json!([owner]));
    let reopened = BridgeState::inspect(&state_root).expect("reopen durable state");
    let saved = reopened.grants().expect("durable explicit grants");
    assert_eq!(
        saved
            .iter()
            .map(|grant| grant.sender_id.as_str())
            .collect::<Vec<_>>(),
        [MEMBER1, MEMBER2]
    );
    assert_eq!(result["grants"], serde_json::to_value(&saved).unwrap());
    for grant in &saved {
        assert!(grant.created_at_millis > 0);
        assert_eq!(
            grant.expires_at_millis - grant.created_at_millis,
            43_200_000
        );
        assert_eq!(grant.note.as_deref(), Some("current channel discussion"));
    }
    assert_eq!(reopened.config().allowed_senders, [owner]);

    // The provider's membership changes after the snapshot. Merely rereading the bridge's
    // authority must not turn that explicit snapshot into ongoing channel membership authority.
    fixture(
        &root,
        "members-first.json",
        &page(&[owner, MEMBER1, MEMBER2, later_member], None),
    );
    let request = reopened.subscribe_request().expect("effective authority");
    assert_eq!(
        request
            .allowed_senders()
            .iter()
            .map(|sender| sender.as_str())
            .collect::<Vec<_>>(),
        [owner, MEMBER1, MEMBER2]
    );
    assert!(!request
        .allowed_senders()
        .iter()
        .any(|sender| matches!(sender.as_str(), "*") || sender.as_str() == later_member));
    assert_eq!(calls(&root).len(), 2);

    fixture(
        &root,
        "resolve.json",
        &json!({"data": {"user_id": resolved_member}}),
    );
    let named = agentctl::chat_service::grant(
        &state_root,
        &["member@example.invalid".to_owned()],
        false,
        Duration::from_secs(43_200),
        Some("named collaborator"),
    )
    .expect("resolve a person through the native helper before granting");
    assert_eq!(named["grants"][0]["sender_id"], resolved_member);
    assert_eq!(named["grants"][0]["note"], "named collaborator");
    let saved = BridgeState::inspect(&state_root)
        .expect("reopen resolved grant")
        .grants()
        .expect("all durable grants");
    assert_eq!(
        saved
            .iter()
            .map(|grant| grant.sender_id.as_str())
            .collect::<Vec<_>>(),
        [MEMBER1, resolved_member, MEMBER2]
    );
    assert_eq!(
        calls(&root).last(),
        Some(&format!("slack.user describe --user=member@example.invalid --columns=user_id --as={APP} --output=json"))
    );
    assert!(!fs::read_to_string(state_root.join("grants.json"))
        .unwrap()
        .contains("member@example.invalid"));
    assert!(
        !root.join("ledger").exists(),
        "directory reads have no write ledger"
    );
}
