//! The whole summary path with the ElevenLabs agent behind it, offline.
//!
//! Two halves, and both are needed.
//!
//! The first drives `ops::summarize_message` and the HTTP route against the in-memory vendor, so
//! the production caching, scope and error handling all execute and the answer's LATENCY FIELD is
//! checked by a client rather than by the code that fills it in.
//!
//! The second drives the real WebSocket client against
//! [`gent_talk::elevenlabs::mock`], which speaks the vendor's protocol on loopback. That is the
//! only offline exercise of the frames themselves — the initiation, the `user_message`, the
//! `agent_response` — and without it the socket module is a set of unit tests over JSON that has
//! never been through a socket.
//!
//! **Neither half has met the live vendor**, and no test in this repository can. The mock agrees
//! with the socket client because both were written from the same reading of the vendor's SDK.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gent_talk::auth::Scope;
use gent_talk::config::{ElevenLabsConfig, Secret};
use gent_talk::elevenlabs::fake::{FakeElevenLabs, KNOWN_AGENT_ID, VALID_API_KEY};
use gent_talk::elevenlabs::http::HttpElevenLabsClient;
use gent_talk::elevenlabs::mock::{MockElevenLabs, MockOptions, Scenario};
use gent_talk::elevenlabs::socket::WebSocketTextChatProvider;
use gent_talk::elevenlabs::{SignedUrlProvider, TextChat, TextChatProvider};
use gent_talk::model::ChannelId;
use gent_talk::ops::{self, SummaryOutcome};
use gent_talk::summarize::agent::{AgentSummarizer, PoolPolicy};
use gent_talk::summarize::Summarizer;
use gent_talk::testing::{READ_CHANNEL, READ_TOKEN};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;

/// Long enough to be over the default summary threshold, so the summariser is actually called.
fn a_wall_of_text(marker: &str) -> String {
    format!(
        "{marker} {}",
        "the mac runner stalled again and nobody noticed. ".repeat(20)
    )
}

fn wired() -> ElevenLabsConfig {
    ElevenLabsConfig {
        agent_id: Some(KNOWN_AGENT_ID.to_owned()),
        api_key: Some(Secret::new(VALID_API_KEY)),
        api_base: gent_talk::config::DEFAULT_ELEVENLABS_API_BASE.to_owned(),
        voice_id: None,
    }
}

/// A server whose summaries come from the agent backend, over the in-memory vendor.
fn server(
    policy: PoolPolicy,
    messages: usize,
) -> (
    gent_talk::state::AppState,
    Arc<FakeElevenLabs>,
    Arc<gent_talk::discord::fake::FakeDiscord>,
) {
    let vendor = Arc::new(FakeElevenLabs::new());
    let summarizer = Arc::new(AgentSummarizer::new(
        Arc::clone(&vendor) as Arc<dyn TextChatProvider>,
        wired(),
        policy,
    ));
    let (state, discord, _store) = gent_talk::testing::state_with_backend(
        &gent_talk::testing::config_toml(),
        summarizer as Arc<dyn Summarizer>,
    );
    let channel = ChannelId(READ_CHANNEL.to_owned());
    for n in 0..messages {
        discord.seed(&channel, "codex-eng", &a_wall_of_text(&format!("m{n}")));
    }
    (state, vendor, discord)
}

async fn newest_ids(state: &gent_talk::state::AppState) -> Vec<String> {
    ops::messages(state, READ_CHANNEL, None)
        .await
        .expect("reads")
        .messages
        .iter()
        .map(|m| m.id.0.clone())
        .collect()
}

#[tokio::test]
async fn a_summary_from_the_agent_comes_back_generated_and_says_how_long_it_took() {
    let (state, vendor, _discord) = server(PoolPolicy::default(), 2);
    let id = newest_ids(&state).await.pop().expect("a message");

    let answer = ops::summarize_message(&state, Scope::Write, READ_CHANNEL, &id, None)
        .await
        .expect("the agent summarises");
    assert!(
        matches!(&answer.outcome, SummaryOutcome::Generated(text) if !text.is_empty()),
        "{:?}",
        answer.outcome
    );
    let took = answer
        .generated_in_ms
        .expect("a generated summary must report what it cost");
    assert!(
        took < 60_000,
        "an in-memory vendor answering in {took} ms means the clock, not the vendor, is wrong"
    );
    assert_eq!(vendor.chats().len(), 1, "one conversation for one summary");
}

#[tokio::test]
async fn a_cache_hit_reports_no_latency_because_no_vendor_was_asked() {
    // A zero here would read as "the vendor answered instantly", which is the opposite of what
    // happened. The distinction is the entire reason the field is an Option.
    let (state, vendor, _discord) = server(PoolPolicy::default(), 2);
    let id = newest_ids(&state).await.pop().expect("a message");

    let first = ops::summarize_message(&state, Scope::Write, READ_CHANNEL, &id, None)
        .await
        .expect("summarises");
    assert!(first.generated_in_ms.is_some());

    let second = ops::summarize_message(&state, Scope::Write, READ_CHANNEL, &id, None)
        .await
        .expect("summarises");
    assert!(matches!(second.outcome, SummaryOutcome::Cached(_)));
    assert_eq!(second.generated_in_ms, None);
    assert_eq!(
        vendor.chats().len(),
        1,
        "the second ask must not open a second conversation"
    );
}

#[tokio::test]
async fn the_summary_route_hands_the_round_trip_time_to_whoever_asked() {
    // The owner's question — how does a round trip to a hosted conversational agent compare with
    // a full-size model — has to be answerable from the API, on the first real run, without
    // anybody instrumenting anything.
    let (state, _vendor, _discord) = server(PoolPolicy::default(), 2);
    let id = newest_ids(&state).await.pop().expect("a message");
    let router = gent_talk::http::router(state);

    let request = Request::builder()
        .method("GET")
        .uri(format!(
            "/api/v1/channels/{READ_CHANNEL}/messages/{id}/summary"
        ))
        .header("authorization", format!("Bearer {READ_TOKEN}"))
        .body(Body::empty())
        .expect("request");
    let response = router.oneshot(request).await.expect("routes");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("json");

    assert_eq!(body["state"], "generated", "{body}");
    assert!(
        body["generated_in_ms"].is_u64(),
        "the answer must carry how long the summariser took: {body}"
    );
    assert!(
        body["backend"]
            .as_str()
            .expect("a backend")
            .contains("ElevenLabs"),
        "a page must never be able to imply a model summary it did not get: {body}"
    );
}

#[tokio::test]
async fn twenty_summaries_do_not_cost_twenty_conversations_nor_one() {
    // The pooling claim, stated as the two failures it sits between: a socket per summary pays a
    // handshake twenty times, and one socket for all twenty writes the twentieth summary in front
    // of nineteen other people's messages.
    let (state, vendor, _discord) = server(PoolPolicy::default(), 22);
    for id in newest_ids(&state).await {
        ops::summarize_message(&state, Scope::Write, READ_CHANNEL, &id, None)
            .await
            .expect("summarises");
    }
    let opened = vendor.chats().len();
    assert!(
        opened > 1 && opened < 22,
        "twenty-two summaries opened {opened} conversations; the pool is doing nothing, or is \
         never being recycled"
    );
    for chat in vendor.chats() {
        assert!(
            chat.asked.len() <= 8,
            "a conversation served {} summaries, so the recycle limit is not being enforced",
            chat.asked.len()
        );
    }
}

#[tokio::test]
async fn the_live_socket_client_really_holds_a_conversation_over_a_real_websocket() {
    // The only offline exercise of the frames themselves. Everything here is production code
    // except the far end: the real mint, the real signed URL, the real socket client.
    let mock = MockElevenLabs::spawn(MockOptions {
        // Answers with canned prose and calls no tools, so this test needs no bridge behind it.
        scenario: Scenario::NoToolCall,
        ..MockOptions::default()
    })
    .await
    .expect("the mock starts");

    let client = Arc::new(HttpElevenLabsClient::new().expect("client"));
    let chats = WebSocketTextChatProvider::new(Arc::clone(&client) as Arc<dyn SignedUrlProvider>);
    let config = ElevenLabsConfig {
        agent_id: Some(gent_talk::elevenlabs::mock::MOCK_AGENT_ID.to_owned()),
        api_key: Some(Secret::new(gent_talk::elevenlabs::mock::MOCK_API_KEY)),
        api_base: mock.api_base(),
        voice_id: None,
    };

    let mut chat = chats
        .open(&config, Duration::from_secs(10))
        .await
        .expect("the conversation opens");

    // Asked of the conversation that was really opened, not of a secret list this test wrote. The
    // minted URL is the entry that is easy to omit: it carries a single-use token and is quoted
    // verbatim by a connect failure, so a conversation that does not hold it cannot scrub it.
    let scrubbed: Vec<&str> = chat
        .scrubbed()
        .iter()
        .map(gent_talk::config::Secret::expose)
        .collect();
    assert!(
        scrubbed.contains(&gent_talk::elevenlabs::mock::MOCK_API_KEY),
        "the account key is not among the secrets this conversation scrubs: {scrubbed:?}"
    );
    assert!(
        scrubbed
            .iter()
            .any(|s| s.starts_with("ws") && s.contains("token=")),
        "the minted URL's single-use token is not among the secrets this conversation scrubs, so \
         a connect failure would print it: {scrubbed:?}"
    );

    let reply = chat
        .ask("summarise this", Duration::from_secs(10))
        .await
        .expect("the agent answers");
    assert!(!reply.trim().is_empty(), "an empty answer is not an answer");
    Box::new(chat).close().await;

    let trace = mock.trace().to_json();
    let kinds: Vec<String> = trace["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter_map(|e| e["kind"].as_str().map(str::to_owned))
        .collect();
    assert!(
        kinds
            .iter()
            .any(|k| k == "conversation_initiation_client_data"),
        "the vendor requires the client to speak first: {kinds:?}"
    );
    assert!(
        kinds.iter().any(|k| k == "user_message"),
        "the question never reached the agent: {kinds:?}"
    );
    assert!(
        !kinds.iter().any(|k| k == "premature_event"),
        "the client wrote to the conversation before it was initiated: {kinds:?}"
    );
    assert!(
        !kinds.iter().any(|k| k == "client_event"),
        "a frame this repository's own vendor model does not understand went out: {kinds:?}"
    );
}

#[tokio::test]
async fn a_vendor_that_refuses_the_conversation_refuses_the_summary_by_name() {
    // The end-to-end shape of "the vendor said no": a 401 at the mint has to arrive at the API as
    // a refusal with its own code, not as a generic failure and not as a summary.
    let mock = MockElevenLabs::spawn(MockOptions {
        scenario: Scenario::MintRejected,
        ..MockOptions::default()
    })
    .await
    .expect("the mock starts");

    let client = Arc::new(HttpElevenLabsClient::new().expect("client"));
    let chats = WebSocketTextChatProvider::new(Arc::clone(&client) as Arc<dyn SignedUrlProvider>);
    let config = ElevenLabsConfig {
        agent_id: Some(gent_talk::elevenlabs::mock::MOCK_AGENT_ID.to_owned()),
        api_key: Some(Secret::new(gent_talk::elevenlabs::mock::MOCK_API_KEY)),
        api_base: mock.api_base(),
        voice_id: None,
    };
    let summarizer = AgentSummarizer::new(
        Arc::new(chats) as Arc<dyn TextChatProvider>,
        config,
        PoolPolicy::default(),
    );
    let (state, discord, _store) = gent_talk::testing::state_with_backend(
        &gent_talk::testing::config_toml(),
        Arc::new(summarizer) as Arc<dyn Summarizer>,
    );
    discord.seed(
        &ChannelId(READ_CHANNEL.to_owned()),
        "codex-eng",
        &a_wall_of_text("refused"),
    );
    let id = newest_ids(&state).await.pop().expect("a message");

    let error = ops::summarize_message(&state, Scope::Write, READ_CHANNEL, &id, None)
        .await
        .expect_err("a rejected key must not produce a summary");
    assert_eq!(error.code(), "summarizer_refused", "{error}");
    assert!(
        !error
            .to_string()
            .contains(gent_talk::elevenlabs::mock::MOCK_API_KEY),
        "the vendor quoted the key back and it survived into the error: {error}"
    );
}
