//! A Slack-only deployment, assembled exactly as `main` assembles one: configuration text with a
//! `[[providers]]` entry of kind `slack`, the provider router built from it, and the HTTP routes
//! answering through the real Slack client against the loopback Slack fake.

use crate::slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, thread, ts, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;
use vibe_talk::upstream_clear::Clearer;

const TEAM: &str = "C0000MAIN1";
const OTHER: &str = "C0000OTHER";

#[test]
fn the_example_config_runs_the_vendor_tail_in_the_server() {
    // `#709 vt-live-tail-rust`: no separate adapter unit remains; the server supervises the
    // vendor CLI itself, so the example configuration is the deploy contract. The vendor CLI's
    // managed runtime (hundreds of tasks, ~1 GiB measured under the retired adapter unit) now
    // shares the server unit's limits: the README's deploy note says to size them for both.
    let example = include_str!("../vibe-talk.example.toml");
    for needed in [
        "tail_command",
        "slack.conversation",
        "{channels}",
        "{catchup}",
    ] {
        assert!(example.contains(needed), "{needed}");
    }
}

fn config_text(api_base: &str, provider_extra: &str) -> String {
    format!(
        r#"
[auth]
read_token = "{read}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{api_base}"
token = "{TOKEN}"
owner_user_id = "UOWNER001"
registered_channels_writable = true
{provider_extra}
[[channels]]
id = "{TEAM}"
label = "team"
writable = true
"#,
        read = vibe_talk::testing::READ_TOKEN,
    )
}

async fn deployment(fake: &Fake) -> AppState {
    deployment_with(fake, "").await
}

/// A deployment whose Slack provider entry carries `provider_extra` lines as well.
async fn deployment_with(fake: &Fake, provider_extra: &str) -> AppState {
    let text = config_text(&fake.base, provider_extra);
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn fixture() -> FakeState {
    let mut state = FakeState::default();
    state.history.insert(
        TEAM.to_owned(),
        vec![
            profiled(
                &ts(20, 0),
                "UADA00001",
                "Ada",
                "second <https://example.com|link>",
            ),
            profiled(&ts(10, 0), "UGRACE001", "Grace", "first"),
        ],
    );
    state.history.insert(
        OTHER.to_owned(),
        vec![profiled(&ts(30, 0), "UADA00001", "Ada", "elsewhere")],
    );
    state
}

#[tokio::test]
async fn a_slack_provider_from_configuration_reads_and_describes_its_channels() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake).await;

    let (status, config) = call(&state, "GET", "/api/v1/client-config", None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["chat_provider_name"], "Slack");
    assert_eq!(config["channels"][0]["provider"], "slack");
    // One provider: the deployment-wide fields are its own, and no per-provider list is sent.
    assert!(config.get("providers").is_none(), "{config}");
    assert_eq!(config["threading_supported"], true);
    assert_eq!(config["owner_author_id"], "UOWNER001");

    let (status, page) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{TEAM}/page"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let contents: Vec<&str> = page["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["content"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(contents, ["first", "second [link](https://example.com)"]);
    assert_eq!(page["messages"][0]["author"], "Grace");
}

#[tokio::test]
async fn a_pasted_slack_link_adds_a_channel_through_the_slack_provider() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake).await;
    let (status, body) = call(
        &state,
        "POST",
        "/api/v1/channels",
        Some(json!({
            "source": format!("https://acme.slack.com/archives/{OTHER}"),
            "label": "other",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["channel"]["id"], OTHER);
    assert_eq!(body["channel"]["provider"], "slack");
    assert_eq!(body["channel"]["writable"], true);
    let (status, page) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{OTHER}/page"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.to_string().contains("elsewhere"));
}

#[tokio::test]
async fn clearing_unread_in_slack_is_offered_only_when_the_deployment_turns_it_on() {
    // `#258 slack-clear-unread`.
    let path = format!("/api/v1/channels/{TEAM}/upstream-clear");
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake).await;
    let (_, config) = call(&state, "GET", "/api/v1/client-config", None).await;
    assert!(config.get("upstream_clear_supported").is_none(), "{config}");
    let (status, refused) = call(&state, "PUT", &path, Some(json!({"mode": "on"}))).await;
    assert!(status.is_client_error(), "{status} {refused}");

    let fake = Fake::start(fixture(), 20).await;
    let state = deployment_with(&fake, "upstream_clear = true\n").await;
    let (_, config) = call(&state, "GET", "/api/v1/client-config", None).await;
    assert_eq!(config["upstream_clear_supported"], true, "{config}");
    let (status, view) = call(&state, "PUT", &path, Some(json!({"mode": "dry_run"}))).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    // Settings words the control with this: "Clear unread in Slack".
    assert_eq!(view["provider_name"], "Slack");
    assert_eq!(view["supported"], true);
    assert_eq!(view["mode"], "dry_run");
    assert_eq!(
        fake.calls("conversations.mark"),
        0,
        "a dry run marks nothing"
    );
}

/// `#307 slack-reply-live`. A Slack read of a channel's newest messages now carries thread replies,
/// but Settings' "Mark read now" pushes the newest message of the conversation's own, as the live
/// stream offers them for clearing, never a thread reply newer than all of them, and reads no
/// thread to find it. Here the newest message roots a thread whose reply is newer, so a read of the
/// newest message with its thread's replies would answer the reply.
#[tokio::test]
async fn mark_read_now_pushes_the_conversations_own_newest_message_never_a_thread_reply() {
    let mut state = fixture();
    let mut list = state.history.remove(TEAM).expect("team");
    let newest = list
        .iter_mut()
        .find(|message| message["ts"] == json!(ts(20, 0)))
        .expect("the newest message");
    thread(
        &mut state,
        TEAM,
        newest,
        &[(ts(50, 0), "a reply newer than every message".to_owned())],
    );
    state.history.insert(TEAM.to_owned(), list);
    state.last_read.insert(TEAM.to_owned(), ts(10, 0));
    let fake = Fake::start(state, 20).await;
    let state = deployment_with(&fake, "upstream_clear = true\n").await;
    tokio::spawn(Clearer::for_state(&state).run());
    // Let the loop read its (empty) settings before any is saved, so that this press is the only
    // push.
    tokio::task::yield_now().await;
    let path = format!("/api/v1/channels/{TEAM}/upstream-clear");
    let (status, view) = call(&state, "PUT", &path, Some(json!({"mode": "on"}))).await;
    assert_eq!(status, StatusCode::OK, "{view}");

    let (status, answer) = call(&state, "POST", &format!("{path}/now"), None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["outcome"], "pushed", "{answer}");
    let newest = vibe_talk::slack::message_id_from_ts(&ts(20, 0)).expect("an id");
    assert_eq!(
        answer["message_id"].as_str(),
        Some(newest.as_str()),
        "the conversation's own newest message, not the reply: {answer}"
    );
    assert_eq!(
        fake.with(|state| state.marks.clone()),
        vec![(TEAM.to_owned(), ts(20, 0))]
    );
    assert_eq!(
        fake.calls("conversations.replies"),
        0,
        "a thread was read to find it"
    );
}

/// `#252 thread-button-state`, through the routes the page uses. A Slack message nobody has
/// answered heads its own thread, named by its `ts` with no replies; the thread opens with the
/// message alone in it; and the page's first send from that thread's box posts into it with
/// `thread_ts` = the message's `ts`, which is what starts the thread at Slack. The page is also told
/// that a Slack reply is a thread reply, so its reply screen offers no separate "new thread" box.
#[tokio::test]
async fn a_slack_message_nobody_has_answered_can_have_its_thread_started_from_the_page() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake).await;

    let (status, config) = call(&state, "GET", "/api/v1/client-config", None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["replies_in_thread"], true, "{config}");

    let (status, main) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{TEAM}/timeline?view=main"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{main}");
    let first = ts(10, 0);
    let message = main["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["content"] == "first")
        .expect("the message nobody has answered")
        .clone();
    // `#427 server-thread-hues`: the one field this server forms on the record, its thread's
    // colour — 210, the first hue, for the channel's earliest thread.
    assert_eq!(
        message["thread"],
        json!({"id": first, "root_message_id": message["id"], "is_root": true,
            "reply_count": 0, "reply_count_exact": true, "hue": 210}),
        "{message}"
    );

    let (status, thread) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{TEAM}/timeline?view=thread&thread_id={first}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    let ids: Vec<&Value> = thread["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| &message["id"])
        .collect();
    assert_eq!(
        ids,
        [&message["id"]],
        "the empty thread is not the message alone"
    );
    assert_eq!(thread["thread"]["id"], first);
    assert_eq!(thread["thread"]["reply_count"], 0);

    let (status, sent) = call(
        &state,
        "POST",
        &format!("/api/v1/channels/{TEAM}/reply"),
        Some(json!({"text": "the first reply", "thread_id": first})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let posts = fake.requests("chat.postMessage");
    assert_eq!(posts.len(), 1);
    assert_eq!(
        posts[0]["thread_ts"], first,
        "the first reply did not start the message's thread"
    );
    assert_eq!(posts[0]["channel"], TEAM);
    assert_eq!(sent["posted"]["thread"]["id"], first);
    assert_eq!(sent["posted"]["thread"]["is_root"], false);
    assert_eq!(sent["posted"]["thread"]["root_message_id"], message["id"]);
}
