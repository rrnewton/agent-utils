//! `POST /api/v1/channels/{id}/glossary/lookup` and `GET /api/v1/glossary/terms`:
//! one WTF press. `#2156 vibe-talk-wtf-glossary-lookup`.
//!
//! Through the real router with only the model stubbed (the FakeSummarizer):
//! evidence comes from the real message cache, the chat fake's read count
//! stays frozen (cache-only), a found term persists as a dynamic entry and
//! then answers from the cache with no second model call.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::auth::Scope;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::testing::{self, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::threads::{TimelinePage, TimelineRequest, TimelineView};

fn base_ms() -> i64 {
    vibe_talk::clock::instant_ms("2026-09-20T00:00:00Z").expect("base instant")
}

fn message(seq: i64, content: &str) -> Message {
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
        content: content.to_owned(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        labels: Vec::new(),
        thread: None,
        reactions: None,
        heading: None,
        mentions: Vec::new(),
        upstream_url: None,
        upstream_app_url: None,
        op_key: None,
        observed_ms: None,
        sent_refs: Vec::new(),
        sent_vestigial: false,
    }
}

#[derive(Default)]
struct Backend {
    messages: Mutex<Vec<Message>>,
    timeline_reads: Mutex<usize>,
}

impl Backend {
    fn reads(&self) -> usize {
        *self.timeline_reads.lock().expect("reads")
    }
}

#[async_trait::async_trait]
impl ChatClient for Backend {
    fn provider_name(&self) -> &str {
        "Glossary Test"
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
        *self.timeline_reads.lock().expect("reads") += 1;
        let all = self.messages.lock().expect("messages").clone();
        let limit = usize::from(request.limit.max(1));
        let end = match &request.before {
            Some(cursor) => all
                .iter()
                .position(|held| held.id.as_str() == cursor)
                .unwrap_or(all.len()),
            None => all.len(),
        };
        let start = end.saturating_sub(limit);
        let page = all[start..end].to_vec();
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

async fn cached_state(
    backend: Arc<Backend>,
    with_backend: bool,
) -> (
    vibe_talk::state::AppState,
    Arc<vibe_talk::summarize::fake::FakeSummarizer>,
    testing::TempDir,
) {
    let dir = testing::TempDir::new("glossary-lookup");
    let store_path = dir.path().join("store.db");
    let text = format!(
        "{}\n[storage]\npath = \"{}\"\n",
        testing::config_toml(),
        store_path.display()
    );
    let (mut state, _discord, _store, summarizer) = testing::state_with_summarizer(&text);
    state.replace_chat(backend);
    let clock = testing::ManualClock::at(base_ms() + 3_600_000);
    let mut state = testing::with_message_cache(state, dir.path(), clock).await;
    state.message_cache.set_serve(true);
    if with_backend {
        state.catchup_summarizer = Some(summarizer.clone());
        state.catchup_model = Some(Arc::from("fake-model"));
    }
    (state, summarizer, dir)
}

async fn fill(state: &vibe_talk::state::AppState) {
    let request = TimelineRequest {
        view: TimelineView::Flat,
        thread_id: None,
        before: None,
        after: None,
        limit: 99,
    };
    let _ = vibe_talk::ops::timeline(state, Scope::Write, WRITE_CHANNEL, &request)
        .await
        .expect("fill read");
    let cache = state.message_cache.cache().expect("installed");
    assert!(cache.quiesce_blocking(std::time::Duration::from_secs(20)));
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

fn lookup_path() -> String {
    format!("/api/v1/channels/{WRITE_CHANNEL}/glossary/lookup")
}

#[tokio::test]
async fn a_lookup_finds_persists_and_then_answers_from_cache() {
    let defining = message(
        1,
        "Family A is the widget test family covering the checkout flow",
    );
    let use_site = message(2, "the Family A test fix landed this morning");
    let source_id = defining.id.as_str().to_owned();
    let backend = backend_with(vec![defining, use_site]);
    let (state, summarizer, _dir) = cached_state(backend.clone(), true).await;
    fill(&state).await;
    summarizer.answer_with(&format!(
        "{{\"term\":\"Family A\",\"definition\":\"the widget test family covering the checkout flow\",\"source_kind\":\"message\",\"source_id\":\"{source_id}\"}}"
    ));
    let app = vibe_talk::http::router(state);
    let reads = backend.reads();
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "the Family A test fix", "context": "the Family A test fix landed"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("found"), "{body}");
    assert_eq!(body["term"], json!("Family A"), "{body}");
    assert_eq!(body["cached"], json!(false), "{body}");
    assert_eq!(body["saved"], json!(true), "{body}");
    assert_eq!(body["source"]["kind"], json!("message"), "{body}");
    assert_eq!(backend.reads(), reads, "lookup must not call upstream");
    assert_eq!(summarizer.prompt_calls().len(), 1);
    // The term is now served for linking, as a dynamic entry.
    let (status, body) = get(&app, "/api/v1/glossary/terms", READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let terms = body["terms"].as_array().expect("terms");
    assert!(
        terms
            .iter()
            .any(|t| t["term"] == json!("Family A") && t["origin"] == json!("dynamic")),
        "{body}"
    );
    // A second press answers from the entry with no new model call.
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "Family A"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["cached"], json!(true), "{body}");
    assert_eq!(summarizer.prompt_calls().len(), 1, "no second model call");
}

#[tokio::test]
async fn a_term_with_no_evidence_is_a_clear_not_found() {
    let backend = backend_with(vec![message(1, "unrelated status note")]);
    let (state, summarizer, _dir) = cached_state(backend.clone(), true).await;
    fill(&state).await;
    let app = vibe_talk::http::router(state);
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "Zanzibar Protocol"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("not_found"), "{body}");
    assert!(
        body["reason"].as_str().is_some_and(|r| !r.is_empty()),
        "{body}"
    );
    assert_eq!(
        summarizer.prompt_calls().len(),
        0,
        "no evidence, no model call"
    );
}

#[tokio::test]
async fn without_a_side_agent_the_lookup_refuses_by_name() {
    let backend = backend_with(vec![message(1, "Family A is the widget family")]);
    let (state, _summarizer, _dir) = cached_state(backend, false).await;
    fill(&state).await;
    let app = vibe_talk::http::router(state);
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "Family A"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(
        body["error"],
        json!("glossary_not_available_on_this_backend"),
        "{body}"
    );
}

#[tokio::test]
async fn a_read_token_cannot_spend_a_lookup() {
    let backend = backend_with(vec![message(1, "Family A is the widget family")]);
    let (state, _summarizer, _dir) = cached_state(backend, true).await;
    let app = vibe_talk::http::router(state);
    let (status, _body) = post(
        &app,
        &lookup_path(),
        Some(READ_TOKEN),
        &json!({"selected": "Family A"}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
