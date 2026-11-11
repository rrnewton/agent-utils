//! `POST /api/v1/channels/{id}/catchup`: one Discuss read. `catchup-side-agent`, slice 1.
//!
//! The window fetch and its read partition are asserted at [`ops::catchup_fetch`], the seam
//! that does not need the context builder; the endpoint is asserted for its shape, its one
//! recorded model call (system = the shipped default), and its refusals. What the builder
//! puts in the prompt — labels, fences, the unread set it reports — is the builder's own
//! test file's business.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::auth::Scope;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::store::StateStore as _;
use vibe_talk::testing::{self, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::threads::{MessageThread, TimelinePage, TimelineRequest, TimelineView};

const THREAD_A: &str = "thread-A";

fn base_ms() -> i64 {
    vibe_talk::clock::instant_ms("2026-09-20T00:00:00Z").expect("base instant")
}

/// One fixture message `seq` seconds after the base, so ids and times agree in order.
fn message(seq: i64, thread: Option<(&str, bool)>, noise: bool) -> Message {
    let ms = base_ms() + seq * 1_000;
    let numeric = MessageId::at_time_ms(ms).numeric().expect("snowflake");
    Message {
        edited_timestamp: None,
        id: MessageId(numeric.to_string()),
        channel_id: ChannelId(WRITE_CHANNEL.to_owned()),
        author: format!("author-{seq}"),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: vibe_talk::clock::iso_from_ms(ms),
        spoken_time: String::new(),
        reply_to: None,
        // A noise fixture carries the default rule's own text: every timeline read re-marks
        // noise from the rules, so the flag alone would not survive the fetch.
        content: if noise {
            "Working…".to_owned()
        } else {
            format!("message {seq}")
        },
        spoken_content: String::new(),
        content_html: String::new(),
        noise,
        thread: thread.map(|(thread_id, is_root)| MessageThread {
            hue: None,
            display_name: None,
            summary: None,
            id: thread_id.to_owned(),
            root_message_id: None,
            is_root,
            broadcast: false,
            reply_count: None,
            reply_count_exact: false,
        }),
        reactions: None,
        heading: None,
        mentions: Vec::new(),
        upstream_url: None,
        upstream_app_url: None,
        op_key: None,
        observed_ms: None,
    }
}

#[derive(Default)]
struct Backend {
    messages: Mutex<Vec<Message>>,
}

#[async_trait::async_trait]
impl ChatClient for Backend {
    fn provider_name(&self) -> &str {
        "Catchup Test"
    }
    fn supports_threading(&self) -> bool {
        true
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        Ok(ChatIdentity {
            id: "7".to_owned(),
            username: "Reader".to_owned(),
            workspace: None,
        })
    }

    async fn fetch_page(
        &self,
        _channel: &ChannelId,
        _limit: u16,
        _before: Option<&MessageId>,
        _after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        Ok(vec![])
    }

    async fn fetch_timeline(
        &self,
        _channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        let all = self.messages.lock().expect("messages").clone();
        let source: Vec<Message> = match request.view {
            TimelineView::Flat => all,
            TimelineView::Main => all
                .into_iter()
                .filter(|held| held.thread.as_ref().is_none_or(|held| held.is_root))
                .collect(),
            TimelineView::Thread => {
                let thread = request.thread_id.as_deref().unwrap_or("");
                all.into_iter()
                    .filter(|held| held.thread.as_ref().is_some_and(|held| held.id == thread))
                    .collect()
            }
            _ => Vec::new(),
        };
        let limit = usize::from(request.limit.max(1));
        let end = match &request.before {
            Some(cursor) => source
                .iter()
                .position(|held| held.id.as_str() == cursor)
                .unwrap_or(source.len()),
            None => source.len(),
        };
        let start = end.saturating_sub(limit);
        let page = source[start..end].to_vec();
        Ok(TimelinePage {
            messages: page.clone(),
            has_threads: true,
            has_more: start > 0,
            next_before: (start > 0)
                .then(|| page.first().map(|held| held.id.as_str().to_owned()))
                .flatten(),
            ..TimelinePage::default()
        })
    }

    async fn post_message(
        &self,
        _channel: &ChannelId,
        _content: &str,
        _reply: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused("read-only fixture".to_owned()))
    }
}

fn backend_with(messages: Vec<Message>) -> Arc<Backend> {
    let backend = Arc::new(Backend::default());
    *backend.messages.lock().expect("messages") = messages;
    backend
}

#[tokio::test]
async fn fetch_windows_to_the_newest_k_and_partitions_read() {
    let (mut state, _discord, store, _summarizer) =
        testing::state_with_summarizer(&testing::config_toml());
    // 120 messages: more than one 99-message page, so the backward walk really walks.
    let messages: Vec<Message> = (0..120).map(|seq| message(seq, None, seq == 119)).collect();
    state.replace_chat(backend_with(messages.clone()));
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    store
        .dismiss(
            &channel,
            &[
                messages[0].id.clone(),   // outside the window below
                messages[118].id.clone(), // inside it
            ],
        )
        .await
        .expect("dismiss");

    let fetched = vibe_talk::ops::catchup_fetch(
        &state,
        Scope::Write,
        WRITE_CHANNEL,
        TimelineView::Flat,
        None,
        3,
    )
    .await
    .expect("fetch");
    // The newest message is noise: hidden and counted, so the window is the three newest
    // visible messages the walk held.
    assert_eq!(fetched.noise_hidden, 1);
    let ids: Vec<&str> = fetched
        .messages
        .iter()
        .map(|held| held.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            messages[116].id.as_str(),
            messages[117].id.as_str(),
            messages[118].id.as_str(),
        ],
        "newest three visible, oldest first"
    );
    assert_eq!(
        fetched.read,
        std::collections::HashSet::from([messages[118].id.clone()]),
        "exactly the dismissed id inside the window is read"
    );
    assert!(!fetched.truncated);
}

#[tokio::test]
async fn fetch_of_a_thread_covers_it_whole() {
    let (mut state, _discord, _store, _summarizer) =
        testing::state_with_summarizer(&testing::config_toml());
    let mut messages: Vec<Message> = (0..10).map(|seq| message(seq, None, false)).collect();
    let threaded: Vec<Message> = (20..25)
        .map(|seq| message(seq, Some((THREAD_A, seq == 20)), false))
        .collect();
    messages.extend(threaded.clone());
    state.replace_chat(backend_with(messages));

    let fetched = vibe_talk::ops::catchup_fetch(
        &state,
        Scope::Write,
        WRITE_CHANNEL,
        TimelineView::Thread,
        Some(THREAD_A),
        2,
    )
    .await
    .expect("fetch");
    let ids: Vec<&str> = fetched
        .messages
        .iter()
        .map(|held| held.id.as_str())
        .collect();
    let want: Vec<&str> = threaded.iter().map(|held| held.id.as_str()).collect();
    assert_eq!(ids, want, "the whole thread, though the window asked for 2");
    assert!(!fetched.truncated);
}

async fn get(app: &axum::Router, path: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("response");
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

async fn post(
    app: &axum::Router,
    path: &str,
    token: Option<&str>,
    body: &Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method("POST").uri(path);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("response");
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
async fn the_endpoint_answers_with_the_fake_text_and_the_contract_shape() {
    let text = testing::config_toml() + "\n[summaries]\ncatchup_window_messages = 2\n";
    let (mut state, _discord, _store, summarizer) = testing::state_with_summarizer(&text);
    let messages: Vec<Message> = (0..5).map(|seq| message(seq, None, false)).collect();
    state.replace_chat(backend_with(messages.clone()));
    state.catchup_summarizer = Some(summarizer.clone());
    state.catchup_model = Some(Arc::from("fake-model"));
    summarizer.answer_with("one point worth knowing");
    let app = vibe_talk::http::router(state);
    // The page learns before any click that Discuss can answer here.
    let (status, config) = get(&app, "/api/v1/client-config", READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["discuss_available"], json!(true), "{config}");
    let path = format!("/api/v1/channels/{WRITE_CHANNEL}/catchup");

    let (status, body) = post(&app, &path, Some(WRITE_TOKEN), &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["text"], json!("one point worth knowing"), "{body}");
    assert_eq!(body["backend"], json!(vibe_talk::summarize::agent::BACKEND));
    assert_eq!(body["model"], json!("fake-model"));
    assert_eq!(body["thread_id"], Value::Null);
    assert_eq!(body["view"], json!("flat"));
    assert_eq!(
        body["window"]["count"],
        json!(2),
        "the configured window: {body}"
    );
    assert_eq!(
        body["window"]["from_id"],
        json!(messages[3].id.as_str()),
        "{body}"
    );
    assert_eq!(
        body["window"]["through_id"],
        json!(messages[4].id.as_str()),
        "{body}"
    );
    assert_eq!(body["window"]["truncated"], json!(false));
    assert!(body["unread_ids"].is_array() && body["markable_ids"].is_array());
    assert!(body["footer_links"].is_array());
    // The stamp: fields present and typed, nothing timed.
    for field in ["context_ms", "model_ms", "total_ms", "input_chars"] {
        assert!(body["stamp"][field].is_number(), "stamp.{field}: {body}");
    }
    assert_eq!(body["stamp"]["output_chars"], json!(23));
    for field in ["first_token_ms", "input_tokens", "output_tokens"] {
        assert_eq!(body["stamp"][field], Value::Null, "stamp.{field}: {body}");
    }
    // Exactly one model call, under the shipped default instruction.
    let calls = summarizer.prompt_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, vibe_talk::catchup::DEFAULT_SYSTEM_PROMPT);

    // Write scope only, and a view this route does not serve is refused by name.
    assert_eq!(
        post(&app, &path, Some(READ_TOKEN), &json!({})).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&app, &path, None, &json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&app, &path, Some(WRITE_TOKEN), &json!({"view": "threads"}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn on_an_elevenlabs_only_deployment_discuss_refuses_by_backend_name() {
    // The test configuration names an [elevenlabs] agent and no [summaries.llm]: summaries
    // are fully configured (through the agent), so Discuss must refuse saying it is not
    // available on that backend — never as a bare summariser-not-configured.
    let (mut state, _discord, _store, _summarizer) =
        testing::state_with_summarizer(&testing::config_toml());
    state.replace_chat(backend_with(vec![message(0, None, false)]));
    assert!(state.catchup_summarizer.is_none());
    let app = vibe_talk::http::router(state);
    // And the page learns so at startup: the capability is absent, so its Discuss button is
    // drawn disabled rather than offered and refused on click.
    let (status, config) = get(&app, "/api/v1/client-config", READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert!(
        config.get("discuss_available").is_none(),
        "absent means unavailable: {config}"
    );

    let (status, body) = post(
        &app,
        &format!("/api/v1/channels/{WRITE_CHANNEL}/catchup"),
        Some(WRITE_TOKEN),
        &json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(
        body["error"],
        json!("discuss_not_available_on_this_backend"),
        "{body}"
    );
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("[summaries.llm]") && detail.contains("ElevenLabs agent"),
        "the refusal names what Discuss needs and the backend in use: {body}"
    );
    // An unknown channel is still a 404 first: the allowlist answers before the backend.
    let (status, _body) = post(
        &app,
        "/api/v1/channels/9999999999/catchup",
        Some(WRITE_TOKEN),
        &json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
