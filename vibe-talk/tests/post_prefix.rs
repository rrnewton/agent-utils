//! `#303 voice-post-prefix`, end to end through the real router.
//!
//! Every message the voice agent posts starts with the owner's tag — by default
//! "(Voice-agent relayed, from user) " — and nothing he types on the page does. Each path by which
//! the agent posts is driven here as a client drives it: MCP's `post_reply` and the REST route a
//! bridge maps it onto, proposed and then committed by a tap or a spoken yes, or sent at once with
//! confirmation off, to the channel or as a reply. The in-memory chat service records what actually
//! went out, so "tagged once" is an observation of the post itself.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt as _;
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::http::router;
use vibe_talk::post_prefix::{DEFAULT_PREFIX, MAX_PREFIX_CHARS};
use vibe_talk::store::fake::FakeStore;
use vibe_talk::store::StateStore as _;
use vibe_talk::testing::{READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};

const TAG: &str = DEFAULT_PREFIX;

struct Harness {
    router: axum::Router,
    discord: Arc<FakeDiscord>,
    store: Arc<FakeStore>,
}

fn harness() -> Harness {
    let (state, discord, store) = vibe_talk::testing::state_with_store();
    Harness {
        router: router(state),
        discord,
        store,
    }
}

/// With "Ask me before the voice agent sends" off.
async fn sending_at_once() -> Harness {
    let harness = harness();
    harness
        .store
        .set_ask_before_send(false)
        .await
        .expect("confirmation off");
    harness
}

async fn send(
    harness: &Harness,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
    let response = harness
        .router
        .clone()
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

/// `post_reply` over MCP with the write token, as a voice agent calls it; the text it is handed.
async fn post_reply(harness: &Harness, arguments: Value) -> String {
    let (status, body) = send(
        harness,
        "POST",
        "/mcp",
        WRITE_TOKEN,
        Some(json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "post_reply", "arguments": arguments },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["isError"], false, "{body}");
    body["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The proposal waiting for the owner, as the voice page reads it.
async fn pending(harness: &Harness) -> Value {
    let (status, body) = send(harness, "GET", "/api/v1/post-proposals", WRITE_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["proposal"].clone()
}

/// Commit `proposal` exactly as the page's Send or spoken yes does: its server copy, restated.
async fn commit(harness: &Harness, proposal: &Value, confirmed_by: &str) -> Value {
    let (status, body) = send(
        harness,
        "POST",
        "/api/v1/post-proposals/commit",
        WRITE_TOKEN,
        Some(json!({
            "handle": proposal["handle"],
            "channel_id": proposal["channel_id"],
            "text": proposal["text"],
            "reply_to": proposal["reply_to"],
            "confirmed_by": confirmed_by,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn posted(harness: &Harness) -> Vec<String> {
    harness
        .discord
        .posted()
        .into_iter()
        .map(|post| post.content)
        .collect()
}

// --- every path by which the voice agent posts ---------------------------------------------------

#[tokio::test]
async fn a_proposal_is_read_back_shown_and_posted_with_the_tag_by_a_tap_or_a_yes() {
    for confirmed_by in ["ui", "speaker_turn"] {
        let harness = harness();
        let said = post_reply(
            &harness,
            json!({ "channel_id": WRITE_CHANNEL, "text": "on my way" }),
        )
        .await;
        // The model reads back what will be posted, tag and all.
        assert!(
            said.contains(&format!("Exact text: {TAG}on my way\n")),
            "{said}"
        );
        assert!(
            said.contains(&format!(
                "Read-back: To lead team: {TAG}on my way. Send it?"
            )),
            "{said}"
        );
        // The card shows the same text, and says which part is the owner's tag.
        let proposal = pending(&harness).await;
        assert_eq!(proposal["text"], format!("{TAG}on my way"), "{proposal}");
        assert_eq!(proposal["prefix"], TAG, "{proposal}");
        assert!(posted(&harness).is_empty(), "a proposal posted");
        // The commit posts that text as it stands: one tag, not two.
        let body = commit(&harness, &proposal, confirmed_by).await;
        assert_eq!(body["posted"]["content"], format!("{TAG}on my way"));
        assert_eq!(posted(&harness), [format!("{TAG}on my way")]);
    }
}

#[tokio::test]
async fn the_rest_route_a_bridge_maps_post_reply_onto_tags_the_same_way() {
    let harness = harness();
    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/post-proposals",
        WRITE_TOKEN,
        Some(json!({ "channel_id": WRITE_CHANNEL, "text": "on my way" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = body["result"].as_str().expect("a result for the model");
    assert!(
        result.contains(&format!("Exact text: {TAG}on my way")),
        "{result}"
    );
    let proposal = pending(&harness).await;
    assert_eq!(proposal["text"], format!("{TAG}on my way"));
    commit(&harness, &proposal, "ui").await;
    assert_eq!(posted(&harness), [format!("{TAG}on my way")]);

    // And with confirmation off, the REST route posts tagged at once.
    let harness = sending_at_once().await;
    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/post-proposals",
        WRITE_TOKEN,
        Some(json!({ "channel_id": WRITE_CHANNEL, "text": "rebasing now" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["sent"], true, "{body}");
    assert_eq!(posted(&harness), [format!("{TAG}rebasing now")]);
}

#[tokio::test]
async fn a_direct_send_and_a_direct_reply_are_posted_tagged_and_said_as_posted() {
    let harness = sending_at_once().await;
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "rebasing now" }),
    )
    .await;
    assert!(said.starts_with("SENT. "), "{said}");
    assert!(
        said.contains(&format!("Exact text posted: {TAG}rebasing now\n")),
        "{said}"
    );
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "on it", "reply_to": "1000" }),
    )
    .await;
    assert!(said.contains("as a reply to message 1000"), "{said}");
    let posts = harness.discord.posted();
    assert_eq!(posts.len(), 2, "{posts:?}");
    assert_eq!(posts[0].content, format!("{TAG}rebasing now"));
    assert_eq!(posts[1].content, format!("{TAG}on it"));
    assert_eq!(
        posts[1].reply_to.as_ref().map(|id| id.as_str()),
        Some("1000")
    );
}

#[tokio::test]
async fn a_proposed_reply_keeps_its_target_and_carries_the_tag() {
    let harness = harness();
    post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "on it", "reply_to": "1000" }),
    )
    .await;
    let proposal = pending(&harness).await;
    assert_eq!(proposal["reply_to"], "1000");
    commit(&harness, &proposal, "speaker_turn").await;
    let posts = harness.discord.posted();
    assert_eq!(posts[0].content, format!("{TAG}on it"));
    assert_eq!(
        posts[0].reply_to.as_ref().map(|id| id.as_str()),
        Some("1000")
    );
}

#[tokio::test]
async fn text_handed_back_with_the_tag_is_tagged_once() {
    // The model was shown the tagged text and passed it straight back: a correction proposed
    // from "Exact text:", and a retried direct send.
    let harness = harness();
    post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": format!("{TAG}on my way") }),
    )
    .await;
    let proposal = pending(&harness).await;
    assert_eq!(proposal["text"], format!("{TAG}on my way"));
    commit(&harness, &proposal, "ui").await;
    assert_eq!(posted(&harness), [format!("{TAG}on my way")]);

    let harness = sending_at_once().await;
    post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "rebasing now" }),
    )
    .await;
    let again = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": format!("{TAG}rebasing now") }),
    )
    .await;
    assert!(again.starts_with("SENT already. "), "{again}");
    assert_eq!(posted(&harness), [format!("{TAG}rebasing now")]);
}

#[tokio::test]
async fn posts_typed_on_the_page_are_never_tagged() {
    let harness = harness();
    let (status, body) = send(
        &harness,
        "POST",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/reply"),
        WRITE_TOKEN,
        Some(json!({ "text": "typed by hand" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(posted(&harness), ["typed by hand"]);
}

// --- the setting ---------------------------------------------------------------------------------

#[tokio::test]
async fn an_empty_tag_turns_it_off_and_a_reset_brings_the_default_back() {
    let harness = sending_at_once().await;
    let (status, shown) = send(
        &harness,
        "PUT",
        "/api/v1/voice-post-prefix",
        WRITE_TOKEN,
        Some(json!({ "prefix": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["prefix"], "");
    assert_eq!(shown["custom"], true, "an empty tag read back as never set");
    post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "plain" }),
    )
    .await;

    // The owner's own tag, used exactly as saved.
    let (status, shown) = send(
        &harness,
        "PUT",
        "/api/v1/voice-post-prefix",
        WRITE_TOKEN,
        Some(json!({ "prefix": "[voice] " })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["prefix"], "[voice] ");
    post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "mine" }),
    )
    .await;

    let (status, shown) = send(
        &harness,
        "DELETE",
        "/api/v1/voice-post-prefix",
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["prefix"], TAG);
    assert_eq!(shown["custom"], false);
    post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "back" }),
    )
    .await;

    assert_eq!(
        posted(&harness),
        [
            "plain".to_owned(),
            "[voice] mine".to_owned(),
            format!("{TAG}back")
        ]
    );
}

#[tokio::test]
async fn with_tagging_off_a_proposal_carries_no_prefix_field() {
    let harness = harness();
    harness.store.set_post_prefix(Some("")).await.expect("off");
    post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "plain" }),
    )
    .await;
    let proposal = pending(&harness).await;
    assert_eq!(proposal["text"], "plain");
    assert!(proposal.get("prefix").is_none(), "{proposal}");
}

#[tokio::test]
async fn the_setting_is_read_at_read_scope_on_its_route_and_in_client_config_and_changed_only_at_write(
) {
    let harness = harness();
    let (status, shown) = send(
        &harness,
        "GET",
        "/api/v1/voice-post-prefix",
        READ_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(
        shown,
        json!({
            "prefix": TAG,
            "default_prefix": TAG,
            "custom": false,
            "max_chars": MAX_PREFIX_CHARS,
            "changeable": true,
        })
    );
    let (status, config) = send(&harness, "GET", "/api/v1/client-config", READ_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["voice_post_prefix"], shown);

    for (method, body) in [("PUT", Some(json!({ "prefix": "" }))), ("DELETE", None)] {
        let (status, answer) = send(
            &harness,
            method,
            "/api/v1/voice-post-prefix",
            READ_TOKEN,
            body,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}: {answer}");
    }
    assert_eq!(harness.store.post_prefix().await.expect("read"), None);

    // No tool reaches it: a model that could take its own tag off could pass its words off as the
    // owner's.
    let (_status, listed) = send(
        &harness,
        "POST",
        "/mcp",
        WRITE_TOKEN,
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })),
    )
    .await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(
        names
            .iter()
            .all(|name| !name.contains("prefix") && !name.contains("tag")),
        "{names:?}"
    );
}

#[tokio::test]
async fn a_tag_over_the_limit_is_refused_and_one_line_is_kept() {
    let harness = harness();
    let (status, answer) = send(
        &harness,
        "PUT",
        "/api/v1/voice-post-prefix",
        WRITE_TOKEN,
        Some(json!({ "prefix": "x".repeat(MAX_PREFIX_CHARS + 1) })),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{answer}");
    assert_eq!(answer["error"], "too_large");
    assert_eq!(harness.store.post_prefix().await.expect("read"), None);

    let (status, shown) = send(
        &harness,
        "PUT",
        "/api/v1/voice-post-prefix",
        WRITE_TOKEN,
        Some(json!({ "prefix": "  [voice]\nrelay " })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["prefix"], "[voice] relay ");
}

#[tokio::test]
async fn a_server_with_no_store_tags_with_the_default_and_cannot_change_it() {
    let (state, discord) =
        vibe_talk::testing::state_with(Arc::new(vibe_talk::store::disabled::DisabledStore));
    let harness = Harness {
        router: router(state),
        discord,
        store: Arc::new(FakeStore::new()),
    };
    let (_status, shown) = send(
        &harness,
        "GET",
        "/api/v1/voice-post-prefix",
        READ_TOKEN,
        None,
    )
    .await;
    assert_eq!(shown["prefix"], TAG);
    assert_eq!(shown["changeable"], false);
    let (status, answer) = send(
        &harness,
        "PUT",
        "/api/v1/voice-post-prefix",
        WRITE_TOKEN,
        Some(json!({ "prefix": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer}");
    post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "on it" }),
    )
    .await;
    let proposal = pending(&harness).await;
    assert_eq!(proposal["text"], format!("{TAG}on it"));
}

// --- the provider's size limit -------------------------------------------------------------------

#[tokio::test]
async fn the_tag_counts_toward_the_limit_and_goes_on_the_first_part_only() {
    // Discord's 2000 characters, which the in-memory chat service keeps.
    const LIMIT: usize = 2_000;
    let room = LIMIT - TAG.chars().count();

    // Words that fill what the tag leaves: one message.
    let harness = sending_at_once().await;
    let fits = "x".repeat(room);
    post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": fits }),
    )
    .await;
    assert_eq!(posted(&harness), [format!("{TAG}{fits}")]);

    // One character more than that, which alone would fit: two, because the tag is counted.
    let harness = sending_at_once().await;
    let over = "y".repeat(room + 1);
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": over }),
    )
    .await;
    let parts = posted(&harness);
    assert_eq!(parts.len(), 2, "{said}");
    assert!(parts.iter().all(|part| part.chars().count() <= LIMIT));
    assert!(parts[0].starts_with(TAG), "{}", parts[0]);
    assert!(
        !parts[1].contains(TAG.trim_end()),
        "the tag was repeated: {}",
        parts[1]
    );
    assert_eq!(
        parts.concat(),
        format!("{TAG}{over}"),
        "a character was lost"
    );
    assert!(
        said.contains("It was long, so it went as 2 messages."),
        "{said}"
    );
}
