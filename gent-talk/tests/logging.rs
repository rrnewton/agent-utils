//! What the access log must contain, and what it must never contain.
//!
//! These tests exist because of a specific incident: a voice agent claimed to have read a Discord
//! channel and posted a reply, and had done neither — it invented a digest and named tools this
//! server does not have. The claim could only be disproved by going and looking at the channel,
//! because the server logged nothing per request, so "the agent never called" and "the agent
//! called and we answered" were the same output. Every assertion below is aimed at making that
//! question answerable from one line of log.
//!
//! Because the whole point is that an absence of evidence was mistaken for evidence, each test
//! that asserts something is ABSENT from the log also asserts that the log was not simply empty.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gent_talk::http::router;
use gent_talk::model::ChannelId;
use gent_talk::testing::{LogCapture, READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;

struct Harness {
    router: axum::Router,
    discord: std::sync::Arc<gent_talk::discord::fake::FakeDiscord>,
}

fn harness() -> Harness {
    let (state, discord) = gent_talk::testing::state();
    Harness {
        router: router(state),
        discord,
    }
}

async fn call(
    harness: &Harness,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> StatusCode {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
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
    let _ = response.into_body().collect().await.expect("body");
    status
}

async fn rpc(harness: &Harness, token: Option<&str>, body: Value) -> StatusCode {
    call(harness, "POST", "/mcp", token, Some(body)).await
}

fn tools_call(name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    })
}

#[tokio::test]
async fn every_request_leaves_exactly_one_line() {
    let capture = LogCapture::info_only();
    let harness = harness();
    assert_eq!(
        call(&harness, "GET", "/api/v1/channels", Some(READ_TOKEN), None).await,
        StatusCode::OK
    );
    let lines: Vec<String> = capture
        .access_lines()
        .into_iter()
        .filter(|l| l.contains("request"))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one request line, got: {lines:?}"
    );
    let line = &lines[0];
    for field in [
        "method=\"GET\"",
        "path=\"/api/v1/channels\"",
        "credential=\"read\"",
        "status=200",
    ] {
        assert!(line.contains(field), "line is missing {field}: {line}");
    }
    assert!(line.contains("millis="), "no duration recorded: {line}");
}

#[tokio::test]
async fn a_refused_request_is_logged_as_loudly_as_an_accepted_one() {
    // The case under investigation is a client that may never have gotten in. A log that only
    // records successful calls cannot tell that story.
    let capture = LogCapture::info_only();
    let harness = harness();
    call(&harness, "GET", "/api/v1/channels", None, None).await;
    call(
        &harness,
        "GET",
        "/api/v1/channels",
        Some("a-stale-token-000000000000"),
        None,
    )
    .await;
    call(&harness, "GET", "/no/such/route", Some(READ_TOKEN), None).await;

    let text = capture.text();
    assert!(
        text.contains("credential=\"absent\"") && text.contains("status=401"),
        "an unauthenticated request left no legible line: {text}"
    );
    assert!(
        text.contains("credential=\"unrecognized\""),
        "a wrong token must be distinguishable from no token at all: {text}"
    );
    assert!(
        text.contains("path=\"/no/such/route\"") && text.contains("status=404"),
        "a request to a path that does not exist -- the most likely shape of a misconfigured \
         client -- was not logged: {text}"
    );
}

#[tokio::test]
async fn an_mcp_tool_call_is_identifiable_by_tool_and_channel() {
    let capture = LogCapture::info_only();
    let harness = harness();
    harness
        .discord
        .seed(&ChannelId(READ_CHANNEL.to_owned()), "codex-eng", "hello");
    assert_eq!(
        rpc(
            &harness,
            Some(READ_TOKEN),
            tools_call("digest_channel", json!({ "channel_id": READ_CHANNEL })),
        )
        .await,
        StatusCode::OK
    );
    let text = capture.text();
    assert!(
        text.contains("rpc_method=\"tools/call\""),
        "the JSON-RPC method is not in the log; every MCP call is a POST /mcp, so without it the \
         log cannot tell tools/list from a real tool call: {text}"
    );
    assert!(
        text.contains("tool=\"digest_channel\""),
        "the tool name is missing: {text}"
    );
    assert!(
        text.contains(&format!("channel=\"{READ_CHANNEL}\"")),
        "the channel is missing: {text}"
    );
    assert!(
        text.contains("outcome=\"ok\""),
        "the outcome is missing: {text}"
    );
}

#[tokio::test]
async fn a_tool_the_server_does_not_have_is_logged_as_refused() {
    // Precisely the confabulation case: the agent named `web_scraper`, which does not exist here.
    // Either it never called, or it called and was refused, and the log has to say which.
    let capture = LogCapture::info_only();
    let harness = harness();
    rpc(
        &harness,
        Some(READ_TOKEN),
        tools_call("web_scraper", json!({ "url": "https://example.test" })),
    )
    .await;
    let text = capture.text();
    assert!(text.contains("tool=\"web_scraper\""), "{text}");
    assert!(text.contains("outcome=\"refused\""), "{text}");
    assert!(text.contains("reason=\"unknown_tool\""), "{text}");
}

#[tokio::test]
async fn a_refusal_records_why_it_was_refused() {
    let capture = LogCapture::info_only();
    let harness = harness();
    // The scope fence.
    rpc(
        &harness,
        Some(READ_TOKEN),
        tools_call(
            "post_reply",
            json!({ "channel_id": WRITE_CHANNEL, "text": "nope" }),
        ),
    )
    .await;
    // An operational rule: a configured but read-only channel.
    rpc(
        &harness,
        Some(WRITE_TOKEN),
        tools_call(
            "post_reply",
            json!({ "channel_id": READ_CHANNEL, "text": "nope" }),
        ),
    )
    .await;
    let text = capture.text();
    assert!(
        text.contains("reason=\"forbidden_scope\""),
        "a scope refusal must name the scope fence: {text}"
    );
    assert!(
        text.contains("reason=\"channel_not_writable\""),
        "an operational refusal must carry its code: {text}"
    );
}

#[tokio::test]
async fn a_post_records_its_length_but_not_its_words_at_info() {
    let capture = LogCapture::info_only();
    let harness = harness();
    let secret_sounding_text = "the-message-body-nobody-should-find-in-a-log";
    assert_eq!(
        rpc(
            &harness,
            Some(WRITE_TOKEN),
            tools_call(
                "post_reply",
                json!({ "channel_id": WRITE_CHANNEL, "text": secret_sounding_text }),
            ),
        )
        .await,
        StatusCode::OK
    );
    let text = capture.text();
    assert!(
        text.contains("tool=\"post_reply\"") && text.contains("outcome=\"ok\""),
        "the log did not record the post at all, so the absence below proves nothing: {text}"
    );
    assert!(
        text.contains(&format!(
            "text_len={}",
            secret_sounding_text.chars().count()
        )),
        "the length is what answers 'did the whole message go through?': {text}"
    );
    assert!(
        !text.contains(secret_sounding_text),
        "channel content reached the INFO log: {text}"
    );
}

#[tokio::test]
async fn message_content_read_out_of_a_channel_never_reaches_the_info_log() {
    let capture = LogCapture::info_only();
    let harness = harness();
    let content = "third-party-text-that-must-not-be-copied-into-an-operator-log";
    harness
        .discord
        .seed(&ChannelId(READ_CHANNEL.to_owned()), "someone", content);
    assert_eq!(
        call(
            &harness,
            "GET",
            &format!("/api/v1/channels/{READ_CHANNEL}/messages"),
            Some(READ_TOKEN),
            None,
        )
        .await,
        StatusCode::OK
    );
    let text = capture.text();
    assert!(
        text.contains(&format!(
            "path=\"/api/v1/channels/{READ_CHANNEL}/messages\""
        )),
        "the read was not logged, so the absence below proves nothing: {text}"
    );
    assert!(!text.contains(content), "channel text leaked into the log");
}

#[tokio::test]
async fn no_credential_ever_appears_in_the_log() {
    // Every path a token can arrive on, at the level a deployment runs at. Not truncated, not
    // hashed, not prefixed: absent.
    let capture = LogCapture::info_only();
    let harness = harness();
    call(&harness, "GET", "/api/v1/channels", Some(READ_TOKEN), None).await;
    call(&harness, "GET", "/api/v1/channels", Some(WRITE_TOKEN), None).await;
    call(
        &harness,
        "GET",
        "/api/v1/channels",
        Some("a-stale-token-000000000000"),
        None,
    )
    .await;
    rpc(
        &harness,
        Some(WRITE_TOKEN),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    )
    .await;
    call(
        &harness,
        "GET",
        "/api/v1/signed-url",
        Some(WRITE_TOKEN),
        None,
    )
    .await;

    let text = capture.text();
    assert!(
        text.contains("credential=\"read\"") && text.contains("credential=\"write\""),
        "the log recorded nothing, so this test would pass on a server that logs nothing: {text}"
    );
    for secret in [
        READ_TOKEN,
        WRITE_TOKEN,
        "a-stale-token-000000000000",
        "test-bot-token",
        gent_talk::elevenlabs::fake::VALID_API_KEY,
    ] {
        assert!(
            !text.contains(secret),
            "a credential reached the log: {text}"
        );
        // A prefix long enough to confirm a guess is a leak too.
        let prefix = &secret[..12.min(secret.len())];
        assert!(
            !text.contains(prefix),
            "a credential PREFIX ({prefix}) reached the log: {text}"
        );
    }
}

#[tokio::test]
async fn tool_arguments_are_available_at_debug_and_only_at_debug() {
    // The content rule is a LEVEL rule, not a "we happen not to log it" rule: the detail exists
    // for debugging, and turning INFO on must not turn it on.
    let content = "content-visible-only-when-debugging";
    let debug = LogCapture::start();
    {
        let harness = harness();
        rpc(
            &harness,
            Some(WRITE_TOKEN),
            tools_call(
                "post_reply",
                json!({ "channel_id": WRITE_CHANNEL, "text": content }),
            ),
        )
        .await;
    }
    assert!(
        debug.text().contains(content),
        "the DEBUG detail line is missing, so the INFO assertion below is vacuous: {}",
        debug.text()
    );
    drop(debug);

    let info = LogCapture::info_only();
    {
        let harness = harness();
        rpc(
            &harness,
            Some(WRITE_TOKEN),
            tools_call(
                "post_reply",
                json!({ "channel_id": WRITE_CHANNEL, "text": content }),
            ),
        )
        .await;
    }
    assert!(
        !info.text().contains(content),
        "content that is supposed to be DEBUG-only appeared at INFO: {}",
        info.text()
    );
}

#[tokio::test]
async fn a_server_nobody_called_logs_nothing_which_is_the_whole_point() {
    // The control for every test above. If a line appeared here, "the log is empty" would stop
    // meaning "nobody called", and the log would be back to answering nothing.
    let capture = LogCapture::info_only();
    let _harness = harness();
    assert!(
        capture.access_lines().is_empty(),
        "building a server emitted access lines, so an empty log would no longer prove that \
         nothing called: {:?}",
        capture.access_lines()
    );
}
