//! `#34 voice-chat-write-confirm`, end to end through the real router.
//!
//! A model's `post_reply` proposes; only the application's commit route posts, and only with the
//! single-use handle, the proposal restated exactly, and a confirmation the model cannot produce.
//! Every test here drives the same HTTP stack a bridge or the voice page does, and the in-memory
//! chat service records what actually went out — so "nothing was posted" is an observation, not
//! an inference from a status code.
//!
//! What the gate LOGS is tested in `tests/logging.rs`, beside every other access-log assertion:
//! a log capture is per thread, and it is reliable only in a binary where every test holds one.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::http::router;
use vibe_talk::model::ChannelId;
use vibe_talk::post_gate::PostGate;
use vibe_talk::testing::{READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};

struct Harness {
    router: axum::Router,
    discord: Arc<FakeDiscord>,
}

fn harness() -> Harness {
    harness_with(PostGate::new())
}

fn harness_with(gate: PostGate) -> Harness {
    let (mut state, discord) = vibe_talk::testing::state();
    state.post_gate = Arc::new(gate);
    Harness {
        router: router(state),
        discord,
    }
}

async fn send(
    harness: &Harness,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    match body {
        Some(json) => {
            let json = json.to_string();
            send_raw(
                harness,
                method,
                uri,
                Some(token),
                Some("application/json"),
                &json,
            )
            .await
        }
        None => send_raw(harness, method, uri, Some(token), None, "").await,
    }
}

/// The model's side: `post_reply` over MCP. Returns the tool result's text.
async fn post_reply(harness: &Harness, token: &str, arguments: Value) -> (StatusCode, Value) {
    send(
        harness,
        "POST",
        "/mcp",
        token,
        Some(json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": { "name": "post_reply", "arguments": arguments },
        })),
    )
    .await
}

fn tool_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The application's side: what is waiting for the speaker, handle included.
async fn pending(harness: &Harness) -> Value {
    let (status, body) = send(harness, "GET", "/api/v1/post-proposals", WRITE_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["proposal"].clone()
}

/// The commit body for `proposal` exactly as proposed.
fn restated(proposal: &Value) -> Value {
    json!({
        "handle": proposal["handle"],
        "channel_id": proposal["channel_id"],
        "text": proposal["text"],
        "reply_to": proposal["reply_to"],
        "confirmed_by": "ui",
    })
}

async fn commit(harness: &Harness, body: Value) -> (StatusCode, Value) {
    send(
        harness,
        "POST",
        "/api/v1/post-proposals/commit",
        WRITE_TOKEN,
        Some(body),
    )
    .await
}

/// `text` as the voice agent's post carries it: the owner's tag in front, by default. `#303
/// voice-post-prefix`.
fn tagged(text: &str) -> String {
    format!("{}{text}", vibe_talk::post_prefix::DEFAULT_PREFIX)
}

/// Propose over MCP with the write token, check nothing went out, and return the pending proposal.
async fn propose(harness: &Harness, text: &str) -> Value {
    let (status, response) = post_reply(
        harness,
        WRITE_TOKEN,
        json!({ "channel_id": WRITE_CHANNEL, "text": text }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert!(tool_text(&response).starts_with("NOT SENT."), "{response}");
    assert!(
        harness.discord.posted().is_empty(),
        "a proposal posted: {:?}",
        harness.discord.posted()
    );
    let proposal = pending(harness).await;
    assert_eq!(proposal["text"], tagged(text), "{proposal}");
    assert_eq!(
        proposal["prefix"],
        vibe_talk::post_prefix::DEFAULT_PREFIX,
        "{proposal}"
    );
    proposal
}

#[tokio::test]
async fn a_write_scope_post_reply_without_confirmation_posts_nothing() {
    let harness = harness();
    let (_status, response) = post_reply(
        &harness,
        WRITE_TOKEN,
        json!({ "channel_id": WRITE_CHANNEL, "text": "ship it" }),
    )
    .await;
    let said = tool_text(&response);
    assert!(said.starts_with("NOT SENT."), "{said}");
    assert!(
        said.contains(&format!("Exact text: {}", tagged("ship it"))),
        "{said}"
    );
    // Asking again is not confirming: it replaces the proposal, and still nothing goes out.
    let (_status, again) = post_reply(
        &harness,
        WRITE_TOKEN,
        json!({ "channel_id": WRITE_CHANNEL, "text": "ship it" }),
    )
    .await;
    assert!(tool_text(&again).starts_with("NOT SENT."), "{again}");
    assert!(
        harness.discord.posted().is_empty(),
        "{:?}",
        harness.discord.posted()
    );
    // The model is never handed the handle that would send it.
    let proposal = pending(&harness).await;
    let handle = proposal["handle"].as_str().expect("a handle");
    assert_eq!(handle.len(), 22, "{proposal}");
    assert!(!said.contains(handle) && !tool_text(&again).contains(handle));
    assert_eq!(proposal["channel_id"], WRITE_CHANNEL);
    assert_eq!(proposal["channel_name"], "lead team");
    assert!(proposal["expires_in_ms"].as_u64().expect("a number") > 0);
}

#[tokio::test]
async fn a_confirmed_handle_posts_exactly_once() {
    let harness = harness();
    let proposal = propose(&harness, "on my way").await;
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["serial"], proposal["serial"]);
    assert_eq!(body["posted"]["content"], tagged("on my way"));
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "proposal_used");
    let posted = harness.discord.posted();
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert_eq!(posted[0].channel.as_str(), WRITE_CHANNEL);
    assert_eq!(posted[0].content, tagged("on my way"));
    assert_eq!(pending(&harness).await, Value::Null);
}

#[tokio::test]
async fn a_confirmed_post_that_fails_to_send_still_spends_the_handle() {
    // The confirmation was for one attempt. A failure is reported and the model can propose again;
    // the old handle does not become a standing licence to retry.
    let harness = harness();
    let proposal = propose(&harness, "on my way").await;
    harness.discord.fail_next("the chat service is down");
    let (status, body) = commit(&harness, restated(&proposal)).await;
    // The send path reports a failed first part as a partial send of nothing.
    assert_eq!(status, StatusCode::MULTI_STATUS, "{body}");
    assert_eq!(body["error"], "partially_posted", "{body}");
    assert_eq!(body["posted"], 0, "{body}");
    assert!(harness.discord.posted().is_empty());
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "proposal_used");
    assert!(harness.discord.posted().is_empty());
    assert_eq!(pending(&harness).await, Value::Null);
}

#[tokio::test]
async fn the_reply_target_is_part_of_what_is_confirmed_and_what_is_posted() {
    let harness = harness();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    harness
        .discord
        .seed(&channel, "alice", "who has the runner");
    let (_status, window) = send(
        &harness,
        "GET",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/messages"),
        READ_TOKEN,
        None,
    )
    .await;
    let target = window["messages"][0]["id"]
        .as_str()
        .expect("the seeded message has an id")
        .to_owned();
    let (_status, response) = post_reply(
        &harness,
        WRITE_TOKEN,
        json!({ "channel_id": WRITE_CHANNEL, "text": "me", "reply_to": target }),
    )
    .await;
    assert!(tool_text(&response).contains(&target), "{response}");
    let proposal = pending(&harness).await;
    assert_eq!(proposal["reply_to"], target.as_str());
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let posted = harness.discord.posted();
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert_eq!(
        posted[0].reply_to.as_ref().map(|id| id.as_str()),
        Some(target.as_str())
    );
}

#[tokio::test]
async fn an_edited_restatement_is_refused_and_spends_the_handle() {
    for (field, edited) in [
        ("text", json!("on my way!")),
        ("channel_id", json!(READ_CHANNEL)),
        ("reply_to", json!("300")),
    ] {
        let harness = harness();
        let proposal = propose(&harness, "on my way").await;
        let mut body = restated(&proposal);
        body[field] = edited;
        let (status, refused) = commit(&harness, body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{field}: {refused}");
        assert_eq!(refused["error"], "proposal_mismatch", "{field}");
        // Guessing what changed cannot turn a refusal into a send: the handle is gone.
        let (status, retried) = commit(&harness, restated(&proposal)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{field}: {retried}");
        assert_eq!(retried["error"], "proposal_used", "{field}");
        assert!(
            harness.discord.posted().is_empty(),
            "{field}: {:?}",
            harness.discord.posted()
        );
    }
}

#[tokio::test]
async fn an_expired_handle_is_refused() {
    let harness = harness_with(PostGate::with_ttl(Duration::from_millis(150)));
    let proposal = propose(&harness, "on my way").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(pending(&harness).await, Value::Null);
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert_eq!(body["error"], "proposal_expired");
    assert!(harness.discord.posted().is_empty());
}

#[tokio::test]
async fn a_superseded_handle_is_refused_and_only_the_current_draft_can_be_sent() {
    let harness = harness();
    let first = propose(&harness, "on my way").await;
    let second = propose(&harness, "running late").await;
    assert_ne!(first["handle"], second["handle"]);
    let (status, body) = commit(&harness, restated(&first)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "proposal_superseded");
    assert!(harness.discord.posted().is_empty());
    let (status, body) = commit(&harness, restated(&second)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let posted = harness.discord.posted();
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].content, tagged("running late"));
}

#[tokio::test]
async fn a_cancelled_handle_is_refused() {
    let harness = harness();
    let proposal = propose(&harness, "on my way").await;
    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/post-proposals/cancel",
        WRITE_TOKEN,
        Some(json!({ "handle": proposal["handle"] })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(pending(&harness).await, Value::Null);
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "proposal_used");
    assert!(harness.discord.posted().is_empty());
}

#[tokio::test]
async fn an_unknown_handle_is_refused_and_disturbs_nothing() {
    let harness = harness();
    let proposal = propose(&harness, "on my way").await;
    let mut forged = restated(&proposal);
    forged["handle"] = json!("AAAAAAAAAAAAAAAAAAAAAA");
    let (status, body) = commit(&harness, forged).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "proposal_unknown");
    assert!(harness.discord.posted().is_empty());
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(harness.discord.posted().len(), 1);
}

#[tokio::test]
async fn a_commit_must_name_a_confirmation_the_server_knows() {
    let harness = harness();
    let proposal = propose(&harness, "on my way").await;
    for confirmed_by in [json!("model"), json!(null), json!("")] {
        let mut body = restated(&proposal);
        body["confirmed_by"] = confirmed_by.clone();
        let (status, _body) = commit(&harness, body).await;
        assert!(
            status.is_client_error(),
            "confirmed_by {confirmed_by} answered {status}"
        );
    }
    assert!(harness.discord.posted().is_empty());
    let mut body = restated(&proposal);
    body["confirmed_by"] = json!("speaker_turn");
    let (status, body) = commit(&harness, body).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(harness.discord.posted().len(), 1);
}

#[tokio::test]
async fn concurrent_commits_of_one_handle_post_once() {
    let harness = Arc::new(harness());
    let proposal = propose(&harness, "on my way").await;
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let harness = Arc::clone(&harness);
        let body = restated(&proposal);
        tasks.push(tokio::spawn(async move { commit(&harness, body).await.0 }));
    }
    let mut ok = 0;
    for task in tasks {
        if task.await.expect("task") == StatusCode::OK {
            ok += 1;
        }
    }
    assert_eq!(ok, 1);
    assert_eq!(harness.discord.posted().len(), 1);
}

#[tokio::test]
async fn the_rest_propose_route_proposes_and_posts_nothing() {
    // The route `post_reply` names as its backing route in the manifest, for a consumer that maps
    // tools onto REST rather than speaking MCP. It must land on the gate too.
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
    assert_eq!(body["sent"], false, "{body}");
    let result = body["result"]
        .as_str()
        .expect("a result sentence for the model");
    assert!(result.starts_with("NOT SENT."), "{result}");
    assert!(
        result.contains(&format!("Exact text: {}", tagged("on my way"))),
        "{result}"
    );
    assert!(harness.discord.posted().is_empty());
    // The answer may be relayed to a model, so it must not carry the capability that commits.
    let (_, pending) = send(&harness, "GET", "/api/v1/post-proposals", WRITE_TOKEN, None).await;
    let handle = pending["proposal"]["handle"]
        .as_str()
        .expect("a pending proposal");
    assert_eq!(pending["proposal"]["serial"], body["serial"]);
    assert!(
        !body.to_string().contains(handle),
        "the proposing answer carried the handle: {body}"
    );
    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/post-proposals",
        WRITE_TOKEN,
        Some(json!({ "channel_id": READ_CHANNEL, "text": "on my way" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "channel_not_writable");
}

#[tokio::test]
async fn the_long_poll_wakes_for_a_proposal_and_for_its_end() {
    let harness = Arc::new(harness());
    let waiter = {
        let harness = Arc::clone(&harness);
        tokio::spawn(async move {
            send(
                &harness,
                "GET",
                "/api/v1/post-proposals?wait=20",
                WRITE_TOKEN,
                None,
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let proposal = propose(&harness, "on my way").await;
    let (status, body) = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("the wait ended when a proposal arrived")
        .expect("task");
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["proposal"]["serial"], proposal["serial"]);

    let serial = proposal["serial"].as_u64().expect("a serial");
    let waiter = {
        let harness = Arc::clone(&harness);
        tokio::spawn(async move {
            send(
                &harness,
                "GET",
                &format!("/api/v1/post-proposals?wait=20&seen={serial}"),
                WRITE_TOKEN,
                None,
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (status, _body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("the wait ended when the proposal was sent")
        .expect("task");
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["proposal"], Value::Null);
}

#[tokio::test]
async fn the_read_token_can_neither_propose_see_nor_confirm() {
    let harness = harness();
    let proposal = propose(&harness, "on my way").await;
    let (status, body) = post_reply(
        &harness,
        READ_TOKEN,
        json!({ "channel_id": WRITE_CHANNEL, "text": "hi" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    for (method, uri, body) in [
        ("GET", "/api/v1/post-proposals", None),
        (
            "POST",
            "/api/v1/post-proposals",
            Some(json!({ "channel_id": WRITE_CHANNEL, "text": "hi" })),
        ),
        (
            "POST",
            "/api/v1/post-proposals/commit",
            Some(restated(&proposal)),
        ),
        (
            "POST",
            "/api/v1/post-proposals/cancel",
            Some(json!({ "handle": proposal["handle"] })),
        ),
    ] {
        let (status, answer) = send(&harness, method, uri, READ_TOKEN, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {answer}");
        assert!(
            !answer.to_string().contains("on my way"),
            "{method} {uri} disclosed the draft: {answer}"
        );
    }
    assert!(harness.discord.posted().is_empty());
    // The refusals left the proposal where it was.
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A request as a careless or hostile caller might send it: any token or none, any content type or
/// none, and a body that need not be JSON at all.
async fn send_raw(
    harness: &Harness,
    method: &str,
    uri: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: &str,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json, text/event-stream");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let request = builder.body(Body::from(body.to_owned())).expect("request");
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

#[tokio::test]
async fn a_caller_without_write_scope_is_refused_before_its_request_is_parsed() {
    // `#41 post-gate-scope-first`. Every route that proposes, confirms, withdraws, reads a
    // proposal, or posts must answer a read token 403 and an anonymous caller 401 — the SAME
    // answer a well-formed request gets — however malformed the request is. Parsing first used to
    // answer 422 or 400 instead, which told a caller with no business here what the route accepts.
    let harness = harness();
    let proposal = propose(&harness, "on my way").await;
    let reply = format!("/api/v1/channels/{WRITE_CHANNEL}/reply");
    let ask = format!("/api/v1/channels/{WRITE_CHANNEL}/ask");
    let well_formed_proposal = json!({ "channel_id": WRITE_CHANNEL, "text": "hi" }).to_string();
    let well_formed_commit = restated(&proposal).to_string();
    let well_formed_cancel = json!({ "handle": proposal["handle"] }).to_string();
    let well_formed_reply = json!({ "text": "hi" }).to_string();
    let well_formed_ask = json!({ "question": "why?" }).to_string();
    // (method, route, a well-formed request, malformed variants of the same route)
    let routes: [(&str, &str, &str, &[&str]); 5] = [
        (
            "POST",
            "/api/v1/post-proposals",
            &well_formed_proposal,
            &["{}", "not json", "{\"text\": 7}"],
        ),
        (
            "POST",
            "/api/v1/post-proposals/commit",
            &well_formed_commit,
            &[
                "{}",
                "not json",
                "{\"handle\": \"x\", \"confirmed_by\": \"the_model\"}",
            ],
        ),
        (
            "POST",
            "/api/v1/post-proposals/cancel",
            &well_formed_cancel,
            &["{}", "[]"],
        ),
        ("POST", &reply, &well_formed_reply, &["{}", "not json"]),
        ("POST", &ask, &well_formed_ask, &["{}"]),
    ];
    for (token, refused) in [
        (Some(READ_TOKEN), StatusCode::FORBIDDEN),
        (None, StatusCode::UNAUTHORIZED),
    ] {
        for (method, uri, well_formed, malformed) in &routes {
            let expected = send_raw(
                &harness,
                method,
                uri,
                token,
                Some("application/json"),
                well_formed,
            )
            .await;
            assert_eq!(expected.0, refused, "{method} {uri}: {}", expected.1);
            let mut variants: Vec<(Option<&str>, &str)> = malformed
                .iter()
                .map(|body| (Some("application/json"), *body))
                .collect();
            // No content type, and the wrong one, are refused by `Json` too when it goes first.
            variants.push((None, well_formed));
            variants.push((Some("text/plain"), well_formed));
            for (content_type, body) in variants {
                let answer = send_raw(&harness, method, uri, token, content_type, body).await;
                assert_eq!(
                    answer, expected,
                    "{method} {uri} with {content_type:?} {body:?} was not refused like a \
                     well-formed request"
                );
            }
        }
        // So is a channel id that does not decode: `Path` refuses `%FF` with 400 when it goes first.
        for (uri, well_formed) in [
            ("/api/v1/channels/%FF/reply", &well_formed_reply),
            ("/api/v1/channels/%FF/ask", &well_formed_ask),
        ] {
            let decodable = uri.replace("%FF", WRITE_CHANNEL);
            let expected = send_raw(
                &harness,
                "POST",
                &decodable,
                token,
                Some("application/json"),
                well_formed,
            )
            .await;
            let answer = send_raw(
                &harness,
                "POST",
                uri,
                token,
                Some("application/json"),
                well_formed,
            )
            .await;
            assert_eq!(
                answer, expected,
                "POST {uri} was not refused like {decodable}"
            );
        }
        // The long poll's query is parsed too, and must not be first either.
        for uri in [
            "/api/v1/post-proposals",
            "/api/v1/post-proposals?wait=soon",
            "/api/v1/post-proposals?wait=5&seen=latest",
        ] {
            let (status, answer) = send_raw(&harness, "GET", uri, token, None, "").await;
            assert_eq!(status, refused, "GET {uri}: {answer}");
            assert!(
                !answer.to_string().contains("on my way"),
                "GET {uri}: {answer}"
            );
        }
    }
    assert!(harness.discord.posted().is_empty());

    // The write token still gets the parser's answer, so the check did not simply swallow
    // malformed requests: they are refused for what they are, and nothing is spent or posted.
    // Exact statuses, so a mistyped route answering 404 or 405 cannot pass for a parser's refusal.
    for uri in [
        "/api/v1/post-proposals",
        "/api/v1/post-proposals/commit",
        "/api/v1/post-proposals/cancel",
        reply.as_str(),
        ask.as_str(),
    ] {
        for (body, parsed) in [
            ("{}", StatusCode::UNPROCESSABLE_ENTITY),
            ("not json", StatusCode::BAD_REQUEST),
        ] {
            let (status, answer) = send_raw(
                &harness,
                "POST",
                uri,
                Some(WRITE_TOKEN),
                Some("application/json"),
                body,
            )
            .await;
            assert_eq!(
                status, parsed,
                "POST {uri} {body:?} with the write token: {answer}"
            );
        }
    }
    let (status, answer) = send_raw(
        &harness,
        "GET",
        "/api/v1/post-proposals?wait=soon",
        Some(WRITE_TOKEN),
        None,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(harness.discord.posted().is_empty());
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the refusals disturbed the proposal: {body}"
    );
}

#[tokio::test]
async fn the_read_scope_tool_list_is_unchanged() {
    // Pinned by name and order, so the gate cannot have added a tool a read-scope bridge sees, or
    // taken one away. The descriptions are pinned against `post_reply` and the gate's routes.
    let harness = harness();
    let (status, body) = send(
        &harness,
        "POST",
        "/mcp",
        READ_TOKEN,
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tools = body["result"]["tools"].as_array().expect("a tool array");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("named"))
        .collect();
    assert_eq!(
        names,
        [
            "recent_activity",
            "list_channels",
            "digest_channel",
            "read_page",
            "count_messages",
            "find_message",
            "read_message",
        ]
    );
    let listing = body.to_string();
    for absent in ["post_reply", "post-proposals", "commit", "proposal"] {
        assert!(!listing.contains(absent), "{absent}: {listing}");
    }
}

// --- #270 voice-transcript-markers: the send memory, through the router --------------------------

/// The long poll, read at once, as a page that reads the send memory reads it.
async fn sends_after(harness: &Harness, after: u64) -> Value {
    let (status, body) = send(
        harness,
        "GET",
        &format!("/api/v1/post-proposals?sent_after={after}"),
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// A message long enough to go as three parts at Discord's limit.
fn three_parts() -> String {
    format!(
        "{}{}{}",
        "alpha ".repeat(250),
        "bravo ".repeat(250),
        "charlie ".repeat(200)
    )
}

/// Another proposal, after something has already been posted.
async fn propose_again(harness: &Harness, text: &str) -> Value {
    let (status, response) = post_reply(
        harness,
        WRITE_TOKEN,
        json!({ "channel_id": WRITE_CHANNEL, "text": text }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert!(tool_text(&response).starts_with("NOT SENT."), "{response}");
    pending(harness).await
}

#[tokio::test]
async fn a_tap_and_a_spoken_yes_each_publish_one_send_saying_how_it_was_approved() {
    let harness = harness();
    let before = sends_after(&harness, 0).await;
    assert!(before.get("sends").is_none(), "{before}");
    assert_eq!(before["sends_through"], 0);
    assert!(
        before["now_ms"].as_i64().is_some_and(|ms| ms > 0),
        "{before}"
    );

    let tapped = propose(&harness, "on my way").await;
    let (status, body) = commit(&harness, restated(&tapped)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["send_seq"], 1, "the commit names its send: {body}");

    let spoken = propose_again(&harness, "right behind you").await;
    let mut confirmed = restated(&spoken);
    confirmed["confirmed_by"] = json!("speaker_turn");
    let (status, body) = commit(&harness, confirmed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["send_seq"], 2);

    let after = sends_after(&harness, 0).await;
    let sends = after["sends"].as_array().expect("sends");
    assert_eq!(sends.len(), 2, "{after}");
    // `#303 voice-post-prefix`: the text that went, the owner's tag in front.
    assert_eq!(sends[0]["text"], tagged("on my way"));
    assert_eq!(sends[0]["detail"]["how"], "tap");
    assert_eq!(sends[0]["detail"]["origin"], "commit");
    assert_eq!(sends[0]["detail"]["serial"], tapped["serial"]);
    assert_eq!(sends[0]["detail"]["outcome"], "sent");
    assert_eq!(sends[0]["detail"]["channel_name"], "lead team");
    assert_eq!(sends[0]["detail"]["parts_posted"], 1);
    assert!(sends[0]["detail"]["message_id"].is_string(), "{after}");
    assert_eq!(sends[1]["detail"]["how"], "voice");
    assert_eq!(after["sends_through"], 2);
    let settled = after["settled"].as_array().expect("settled");
    assert!(
        settled.iter().all(|entry| entry["fate"] == "committed"),
        "{after}"
    );
    // Past the cursor, nothing again; and without a cursor, never any sends.
    assert!(sends_after(&harness, 2).await.get("sends").is_none());
    let (_, plain) = send(&harness, "GET", "/api/v1/post-proposals", WRITE_TOKEN, None).await;
    assert!(plain.get("sends").is_none(), "{plain}");
    assert_eq!(plain["sends_through"], 2);
}

#[tokio::test]
async fn a_partial_commit_answers_207_with_its_serial_and_files_the_parts_that_went() {
    let harness = harness();
    let long = three_parts();
    let proposal = propose(&harness, &long).await;
    // The second part's request never completes: it may or may not be in the channel.
    harness.discord.fail_post_after(1, "connection reset");
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{body}");
    assert_eq!(body["posted"], 1, "{body}");
    assert_eq!(body["uncertain"], true, "{body}");
    assert_eq!(body["serial"], proposal["serial"], "{body}");
    assert_eq!(body["send_seq"], 1, "{body}");

    let after = sends_after(&harness, 0).await;
    let detail = &after["sends"][0]["detail"];
    assert_eq!(detail["outcome"], "partial", "{after}");
    assert_eq!(detail["parts_posted"], 1);
    assert_eq!(detail["parts_total"], 3);
    assert_eq!(detail["uncertain_part"], 2);
    let first = harness.discord.posted()[0].content.clone();
    assert_eq!(after["sends"][0]["text"], first.as_str(), "only what went");
    assert!(
        detail["message_id"].is_string(),
        "the first part can be opened: {after}"
    );
    let unsent = detail["unsent_chars"].as_u64().expect("a count");
    assert!(unsent > 0 && unsent < 1000, "the third part only: {unsent}");
}

#[tokio::test]
async fn a_commit_whose_first_answer_was_lost_is_unconfirmed_and_one_refused_is_not_filed() {
    let harness = harness();
    let proposal = propose(&harness, "on my way").await;
    harness.discord.lose_answer_after(0, "timed out");
    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{body}");
    assert_eq!(body["posted"], 0, "{body}");
    assert_eq!(body["uncertain"], true, "{body}");
    let after = sends_after(&harness, 0).await;
    assert_eq!(
        after["sends"][0]["detail"]["outcome"], "unconfirmed",
        "{after}"
    );
    assert_eq!(after["sends"][0]["text"], tagged("on my way"));

    let refused = propose_again(&harness, "not this one").await;
    harness
        .discord
        .refuse_post_after(0, 400, "{\"message\":\"no\"}");
    let (status, body) = commit(&harness, restated(&refused)).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{body}");
    assert_eq!(body["uncertain"], false, "{body}");
    assert!(body.get("send_seq").is_none(), "{body}");
    assert_eq!(
        sends_after(&harness, 0).await["sends_through"],
        1,
        "a post that certainly went nowhere was filed"
    );
}

#[tokio::test]
async fn the_long_poll_wakes_for_a_send_only_when_it_reads_the_send_memory() {
    let harness = Arc::new(harness());
    let proposal = propose(&harness, "on my way").await;
    let serial = proposal["serial"].as_u64().expect("serial");
    let reader = {
        let harness = Arc::clone(&harness);
        tokio::spawn(async move {
            send(
                &harness,
                "GET",
                &format!("/api/v1/post-proposals?wait=5&seen={serial}&sent_after=0"),
                WRITE_TOKEN,
                None,
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    // A direct send changes nothing pending; only the send memory moves.
    let (status, _) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = tokio::time::timeout(Duration::from_secs(3), reader)
        .await
        .expect("the wait ended")
        .expect("task");
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["sends"][0]["text"], tagged("on my way"), "{body}");
}

#[tokio::test]
async fn a_proposal_names_who_it_answers_and_never_the_id_when_this_server_holds_the_message() {
    let harness = harness();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    harness.discord.seed(
        &channel,
        "Alex",
        "is the build green yet <@1234567890123> because the release is waiting on it",
    );
    let (_status, window) = send(
        &harness,
        "GET",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/messages"),
        WRITE_TOKEN,
        None,
    )
    .await;
    let target = window["messages"][0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let (_status, _response) = post_reply(
        &harness,
        WRITE_TOKEN,
        json!({ "channel_id": WRITE_CHANNEL, "text": "yes", "reply_to": target }),
    )
    .await;
    let proposal = pending(&harness).await;
    assert_eq!(proposal["reply_to_author"], "Alex", "{proposal}");
    let excerpt = proposal["reply_to_excerpt"].as_str().expect("an excerpt");
    assert_eq!(excerpt, "is the build green yet because the release…");
    assert!(
        !excerpt.chars().any(|c| c.is_ascii_digit()),
        "an id reached the card: {excerpt}"
    );

    let (status, body) = commit(&harness, restated(&proposal)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail = sends_after(&harness, 0).await["sends"][0]["detail"].clone();
    assert_eq!(detail["reply_to"], target.as_str());
    assert_eq!(detail["reply_to_author"], "Alex");
    assert_eq!(detail["reply_to_excerpt"], excerpt);

    // A reply to a message this server does not hold is still proposed, with nothing to say.
    let (_status, _response) = post_reply(
        &harness,
        WRITE_TOKEN,
        json!({ "channel_id": WRITE_CHANNEL, "text": "yes", "reply_to": "300" }),
    )
    .await;
    let unknown = pending(&harness).await;
    assert!(unknown.get("reply_to_author").is_none(), "{unknown}");
    assert!(unknown.get("reply_to_excerpt").is_none(), "{unknown}");
}

#[tokio::test]
async fn forgetting_the_stored_conversations_or_purging_empties_the_send_memory() {
    for route in ["/api/v1/conversations", "/api/v1/storage"] {
        let (state, discord, _store) = vibe_talk::testing::state_with_store();
        let harness = Harness {
            router: router(state),
            discord,
        };
        let proposal = propose(&harness, "on my way").await;
        let (status, _) = commit(&harness, restated(&proposal)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            sends_after(&harness, 0).await["sends"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        let (status, body) = send(&harness, "DELETE", route, WRITE_TOKEN, None).await;
        assert_eq!(status, StatusCode::OK, "{route}: {body}");
        let after = sends_after(&harness, 0).await;
        assert!(after.get("sends").is_none(), "{route} left a send: {after}");
        assert_eq!(after["sends_through"], 1, "the counter never goes back");
    }
}

// --- #270 voice-transcript-markers: claims and markers through the append route ------------------

fn store_harness() -> Harness {
    let (state, discord, _store) = vibe_talk::testing::state_with_store();
    Harness {
        router: router(state),
        discord,
    }
}

async fn record(harness: &Harness, conversation: &str, body: Value) -> (StatusCode, Value) {
    send(
        harness,
        "POST",
        &format!("/api/v1/conversations/{conversation}/turns"),
        WRITE_TOKEN,
        Some(body),
    )
    .await
}

async fn stored(harness: &Harness, conversation: &str, records: bool) -> Vec<Value> {
    let query = if records { "?records=1" } else { "" };
    let (status, body) = send(
        harness,
        "GET",
        &format!("/api/v1/conversations/{conversation}{query}"),
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["turns"].as_array().expect("turns").clone()
}

#[tokio::test]
async fn a_claim_files_the_servers_own_copy_of_the_send_once() {
    let harness = store_harness();
    let proposal = propose(&harness, "on my way").await;
    let mut confirmed = restated(&proposal);
    confirmed["confirmed_by"] = json!("speaker_turn");
    let (status, committed) = commit(&harness, confirmed).await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    let send_seq = committed["send_seq"].clone();

    // The page's words are never the record of a send.
    let (status, body) = record(
        &harness,
        "conv_s",
        json!({ "speaker": "sent", "send": send_seq, "text": "something else entirely" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "bad_record");

    let (status, body) = record(
        &harness,
        "conv_s",
        json!({ "speaker": "sent", "send": send_seq, "text": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["turn"]["speaker"], "sent");
    // The server's own copy: what went, the owner's tag included. `#303 voice-post-prefix`.
    assert_eq!(body["turn"]["text"], tagged("on my way"));
    assert_eq!(body["turn"]["detail"]["kind"], "sent");
    assert_eq!(body["turn"]["detail"]["how"], "voice");
    assert_eq!(body["turn"]["detail"]["serial"], proposal["serial"]);
    assert_eq!(body["turn"]["detail"]["channel_name"], "lead team");

    let (status, body) = record(
        &harness,
        "conv_s",
        json!({ "speaker": "sent", "send": send_seq }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "send_recorded");
    let (status, body) = record(&harness, "conv_s", json!({ "speaker": "sent", "send": 99 })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "send_unknown");
    let (status, body) = record(&harness, "conv_s", json!({ "speaker": "sent" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // A read that asked for records gets the row and its detail; one that did not, neither.
    let with = stored(&harness, "conv_s", true).await;
    assert_eq!(with.len(), 1);
    assert_eq!(with[0]["detail"]["outcome"], "sent");
    assert!(stored(&harness, "conv_s", false).await.is_empty());
}

#[tokio::test]
async fn markers_and_answers_are_recorded_only_in_the_shapes_this_server_knows() {
    let harness = store_harness();
    let cases = [
        (
            json!({ "speaker": "agent", "text": "hi", "answer": 3 }),
            "an answer on an agent row",
        ),
        (
            json!({ "speaker": "you", "text": "yes", "send": 1 }),
            "a send on a you row",
        ),
        (
            json!({ "speaker": "you", "text": "yes", "answer": 1_000_001 }),
            "an answer out of range",
        ),
        (
            json!({ "speaker": "marker", "text": "" }),
            "a marker row with no marker",
        ),
        (
            json!({ "speaker": "marker", "text": "words", "marker": { "kind": "not_sent", "why": "lapsed", "serial": 4 } }),
            "text on a marker row",
        ),
        (
            json!({ "speaker": "marker", "marker": { "kind": "not_sent", "why": "bored", "serial": 4 } }),
            "an unknown reason",
        ),
        (
            json!({ "speaker": "marker", "marker": { "kind": "not_sent", "why": "lapsed", "serial": 4, "extra": 1 } }),
            "an unknown field",
        ),
        (
            json!({ "speaker": "marker", "marker": { "kind": "conversation_start", "recap": { "state": "maybe" } } }),
            "an unknown recap state",
        ),
        (
            json!({ "speaker": "marker", "marker": { "kind": "conversation_start", "recap": { "state": "sent", "words": 5_000_000, "lines": 1, "dropped": 0, "transport": "contextual_update" } } }),
            "a number out of range",
        ),
        (
            json!({ "speaker": "marker", "marker": { "kind": "conversation_start", "recap": { "state": "none", "why": "off", "dropped": 0, "words": 3 } } }),
            "a field from the other recap state",
        ),
        (
            json!({ "speaker": "marker", "marker": { "kind": "applause" } }),
            "an unknown kind",
        ),
    ];
    for (body, case) in cases {
        let (status, answer) = record(&harness, "conv_m", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{case}: {answer}");
        assert_eq!(answer["error"], "bad_record", "{case}: {answer}");
    }
    assert_eq!(
        send(
            &harness,
            "GET",
            "/api/v1/conversations/conv_m",
            WRITE_TOKEN,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
        "a refused record created the conversation"
    );

    for body in [
        json!({ "speaker": "marker", "marker": { "kind": "conversation_start", "recap": { "state": "none", "why": "unsupported", "dropped": 0 } } }),
        json!({ "speaker": "agent", "text": "To lead team: ship it. Send it?" }),
        json!({ "speaker": "you", "text": "yes please", "answer": 7 }),
        json!({ "speaker": "marker", "marker": { "kind": "conversation_start", "recap": { "state": "sent", "words": 412, "lines": 9, "dropped": 0, "transport": "client_data" }, "previous_end": "closed" } }),
        json!({ "speaker": "marker", "marker": { "kind": "not_sent", "why": "withdrawn", "serial": 8 } }),
    ] {
        let (status, answer) = record(&harness, "conv_m", body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}: {answer}");
    }
    let with = stored(&harness, "conv_m", true).await;
    assert_eq!(with.len(), 5);
    assert_eq!(with[0]["detail"]["recap"]["why"], "unsupported");
    assert_eq!(with[2]["detail"], json!({ "kind": "answer", "serial": 7 }));
    assert_eq!(with[3]["detail"]["previous_end"], "closed");
    assert_eq!(with[3]["detail"]["recap"]["words"], 412);
    assert_eq!(with[4]["detail"]["why"], "withdrawn");
    let without = stored(&harness, "conv_m", false).await;
    assert_eq!(without.len(), 2, "{without:?}");
    assert!(without.iter().all(|turn| turn.get("detail").is_none()));
    let (_, listed) = send(&harness, "GET", "/api/v1/conversations", WRITE_TOKEN, None).await;
    assert_eq!(
        listed["conversations"][0]["preview"], "To lead team: ship it. Send it?",
        "a marker became the preview: {listed}"
    );
}

#[tokio::test]
async fn forgetting_one_conversation_forgets_the_sends_filed_into_it() {
    let harness = store_harness();
    for (text, conversation) in [("first", "conv_a"), ("second", "conv_b")] {
        let proposal = if text == "first" {
            propose(&harness, text).await
        } else {
            propose_again(&harness, text).await
        };
        let (status, committed) = commit(&harness, restated(&proposal)).await;
        assert_eq!(status, StatusCode::OK, "{committed}");
        let (status, body) = record(
            &harness,
            conversation,
            json!({ "speaker": "sent", "send": committed["send_seq"] }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, _) = send(
        &harness,
        "DELETE",
        "/api/v1/conversations/conv_a",
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let after = sends_after(&harness, 0).await;
    let left: Vec<&str> = after["sends"]
        .as_array()
        .expect("sends")
        .iter()
        .filter_map(|send| send["text"].as_str())
        .collect();
    assert_eq!(left, [tagged("second")], "{after}");
}
