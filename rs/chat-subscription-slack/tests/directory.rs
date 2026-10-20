//! The read-only directory extension, through the real helper process and a fake Slack client.

use std::fs;
use std::io::Write;
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
    cat "$FAKE_SLACK_DIR/resolve.json" ;;
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
