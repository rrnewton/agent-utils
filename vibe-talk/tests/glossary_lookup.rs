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
        plain_labels: Vec::new(),
        manual_labels: Vec::new(),
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
    cached_state_with(backend, with_backend, "").await
}

/// [`cached_state`] with extra configuration appended (e.g. a `[summaries]` table).
async fn cached_state_with(
    backend: Arc<Backend>,
    with_backend: bool,
    extra: &str,
) -> (
    vibe_talk::state::AppState,
    Arc<vibe_talk::summarize::fake::FakeSummarizer>,
    testing::TempDir,
) {
    let dir = testing::TempDir::new("glossary-lookup");
    let store_path = dir.path().join("store.db");
    let text = format!(
        "{}\n[storage]\npath = \"{}\"\n{extra}",
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

/// Synthetic shape of #2189 (no live data): the definition uses the spaced
/// form in the selection's own thread; the selection uses the hyphenated
/// form and names shared link-target tokens; newer generic "stack" noise
/// would otherwise fill the candidate cap.
fn threaded_message(seq: i64, thread_id: &str, is_root: bool, content: &str) -> Message {
    let mut held = message(seq, content);
    held.thread = Some(vibe_talk::threads::MessageThread {
        class: None,
        hue: None,
        display_name: None,
        summary: None,
        id: thread_id.to_owned(),
        root_message_id: None,
        is_root,
        broadcast: false,
        reply_count: None,
        reply_count_exact: false,
        unread_count: None,
        unread_exact: false,
        last_reply_at: None,
        last_reply_author: None,
        last_reply_author_id: None,
        last_reply_excerpt: None,
        participants: Vec::new(),
        participants_more: 0,
        pinned: None,
        root_done: None,
        replies_done: None,
    });
    held
}

#[tokio::test]
async fn a_hyphenated_selection_finds_the_spaced_definition_in_its_thread() {
    let thread = "thread-wtf-fixture";
    let defining = threaded_message(
        1,
        thread,
        true,
        "Memory safety in guest-memory access, which makes record and replay trustworthy: Alpha fix (TKT-101), Beta fix (TKT-102), Gamma fix (TKT-103)",
    );
    let use_site = threaded_message(
        2,
        thread,
        false,
        "Memory-safety stack: the bottom three (TKT-101, TKT-102, TKT-103) wait for review",
    );
    let defining_id = defining.id.as_str().to_owned();
    let use_site_id = use_site.id.as_str().to_owned();
    let mut msgs = vec![defining, use_site];
    for i in 3..15 {
        msgs.push(message(i, &format!("unrelated stack cleanup note {i}")));
    }
    let backend = backend_with(msgs);
    let (state, summarizer, _dir) = cached_state(backend.clone(), true).await;
    fill(&state).await;
    summarizer.answer_with(&format!(
        "{{\"term\":\"Memory safety\",\"definition\":\"guest-memory access safety that makes record and replay trustworthy\",\"source_kind\":\"message\",\"source_id\":\"{defining_id}\"}}"
    ));
    let app = vibe_talk::http::router(state);
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "Memory-safety stack", "message_id": use_site_id, "context": "Memory-safety stack: the bottom three (TKT-101, TKT-102, TKT-103) wait for review"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("found"), "{body}");
    let calls = summarizer.prompt_calls();
    assert_eq!(calls.len(), 1, "one model step");
    assert!(
        calls[0].1.contains("guest-memory access"),
        "definition must reach the model: {}",
        calls[0].1
    );
}

#[tokio::test]
async fn a_learned_entry_stays_out_of_the_discuss_warmup() {
    // Owner-decision guard (#2189): WTF-learned definitions are popover-only
    // until the owner decides otherwise; Discuss must never see them.
    const { assert!(!vibe_talk::glossary_dynamic::DYNAMIC_IN_WARMUP) };
    let defining = message(
        1,
        "Memory safety in guest-memory access, which makes record and replay trustworthy: Alpha fix, Beta fix, Gamma fix",
    );
    let use_site = message(2, "the Memory safety test fix landed this morning");
    let source_id = defining.id.as_str().to_owned();
    let backend = backend_with(vec![defining, use_site]);
    let (state, summarizer, _dir) = cached_state(backend.clone(), true).await;
    fill(&state).await;
    summarizer.answer_with(&format!(
        "{{\"term\":\"Memory safety\",\"definition\":\"guest-memory access safety that makes record and replay trustworthy\",\"source_kind\":\"message\",\"source_id\":\"{source_id}\"}}"
    ));
    let app = vibe_talk::http::router(state);
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "Memory safety"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("found"), "{body}");
    let before = summarizer.prompt_calls().len();
    summarizer.answer_with("a short summary");
    let path = format!("/api/v1/channels/{WRITE_CHANNEL}/catchup");
    let (status, _body) = post(&app, &path, Some(WRITE_TOKEN), &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{_body}");
    let calls = summarizer.prompt_calls();
    assert!(calls.len() > before, "Discuss must have run: {calls:?}");
    for (_, prompt) in &calls[before..] {
        assert!(
            !prompt.contains("access safety that makes"),
            "learned definition leaked into Discuss: {prompt}"
        );
        assert!(
            !prompt.contains("learned from readers"),
            "dynamic warm-up block leaked into Discuss: {prompt}"
        );
    }
}

#[tokio::test]
async fn link_keys_are_real_targets_only_and_a_miss_says_so_plainly() {
    // Author-handle noise ("claude-integ8" from a header line) must never
    // become a link key; a real id (TKT-101) must. The miss names the
    // human search forms and notes there was nothing to follow.
    let backend = backend_with(vec![message(
        1,
        "TKT-101 is the tracking ticket for the rollout",
    )]);
    let (state, summarizer, _dir) = cached_state(backend.clone(), true).await;
    fill(&state).await;
    summarizer.answer_with("{\"term\": null}");
    let app = vibe_talk::http::router(state);
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "Zanzibar Protocol", "context": "claude-integ8 posted about TKT-101 and the Zanzibar Protocol"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("not_found"), "{body}");
    let reason = body["reason"].as_str().expect("reason");
    assert!(reason.contains("\"Zanzibar Protocol\""), "{reason}");
    assert!(!reason.contains("claude-integ8"), "{reason}");
    assert!(!reason.contains("Zanzibarprotocol"), "{reason}");
    assert!(reason.contains("followed the links"), "{reason}");
    assert!(!reason.contains("keywords ("), "{reason}");
    // The TKT-101 key was actually searched: its message reached the model.
    let calls = summarizer.prompt_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].1.contains("tracking ticket"), "{}", calls[0].1);
}

#[tokio::test]
async fn a_long_selection_finds_its_head_phrase_in_the_channels_earlier_status() {
    // `#2232 vibe-talk-wtf-lookup-log-and-replay`: the first logged miss. A
    // six-word selection "Row 7, the <qualifier> gate test" in a top-level
    // status post (no thread, no links). "Row 7" is defined by an earlier
    // top-level status in the same channel that never repeats the
    // qualifier; a "Row 70" line is a decoy that a substring search would
    // also match. The head noun phrase must be searched, in the channel's
    // own recent history, on word boundaries.
    let defining = message(
        1,
        "Status table. Row 7: widget build on the baseline image, owned by team blue",
    );
    let decoy = message(2, "Row 70: packaging check for the sample installer");
    let noise_a = message(3, "the nightly gate is slow again; rerunning the test");
    let noise_b = message(4, "test plan for the gate rewrite is in review");
    let use_site = message(
        5,
        "Status: Row 7, the alpha-12-v3 gate test is green after the retry",
    );
    let source_id = defining.id.as_str().to_owned();
    let decoy_id = decoy.id.as_str().to_owned();
    let use_site_id = use_site.id.as_str().to_owned();
    let backend = backend_with(vec![defining, decoy, noise_a, noise_b, use_site]);
    let (state, summarizer, dir) = cached_state(backend.clone(), true).await;
    fill(&state).await;
    summarizer.answer_with(&format!(
        "{{\"term\":\"Row 7\",\"definition\":\"the widget build on the baseline image\",\"source_kind\":\"message\",\"source_id\":\"{source_id}\"}}"
    ));
    let app = vibe_talk::http::router(state);
    let reads = backend.reads();
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({
            "selected": "Row 7, the alpha-12-v3 gate test",
            "message_id": use_site_id,
            "context": "Status: Row 7, the alpha-12-v3 gate test is green after the retry",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(backend.reads(), reads, "lookup must not call upstream");
    let calls = summarizer.prompt_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let prompt = &calls[0].1;
    assert!(
        prompt.contains(&format!("id={source_id}")),
        "the defining status must reach the evidence: {prompt}"
    );
    assert!(
        !prompt.contains(&format!("id={decoy_id}")),
        "the Row 70 decoy must not: {prompt}"
    );
    assert_eq!(body["state"], json!("found"), "{body}");
    assert_eq!(body["term"], json!("Row 7"), "{body}");
    assert_eq!(body["source"]["kind"], json!("message"), "{body}");
    // The log shows why: the head was split off and found in the channel's
    // own recent history, inside the query budget.
    let records = vibe_talk::glossary_lookup_log::read_records(dir.path());
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(record.outcome, "found");
    assert_eq!(record.head, "Row 7");
    assert_eq!(record.qualifier.as_deref(), Some("alpha-12-v3 gate test"));
    assert_eq!(record.candidates[0].id, source_id, "{record:?}");
    assert_eq!(record.candidates[0].bucket, "channel_recent", "{record:?}");
    assert!(record.candidates[0].head_match);
    assert!(
        !record.candidates.iter().any(|c| c.id == decoy_id),
        "{record:?}"
    );
    assert!(record.queries.len() <= vibe_talk::glossary_query::QUERY_BUDGET);
    assert_eq!(
        record.source.as_ref().map(|s| s.channel.as_str()),
        Some(WRITE_CHANNEL)
    );
    assert_eq!(record.message_id.as_deref(), Some(use_site_id.as_str()));
}

fn lookup_log_mode(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .expect("exists")
        .permissions()
        .mode()
        & 0o777
}

#[tokio::test]
async fn every_lookup_outcome_is_logged_privately_and_the_switch_turns_it_off() {
    // `#2232`: a miss and a refused request each leave one record, in a 0700
    // directory of 0600 day files under the state directory.
    let backend = backend_with(vec![message(1, "nothing relevant here")]);
    let (state, _summarizer, dir) = cached_state(backend.clone(), true).await;
    fill(&state).await;
    let app = vibe_talk::http::router(state);
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "Quokka Ledger", "context": "the Quokka Ledger moved"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("not_found"), "{body}");
    let (status, _) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "  "}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let log_dir = vibe_talk::glossary_lookup_log::log_dir(dir.path());
    assert_eq!(lookup_log_mode(&log_dir), 0o700);
    let files = vibe_talk::glossary_lookup_log::day_files(&log_dir);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(lookup_log_mode(&files[0]), 0o600);
    let records = vibe_talk::glossary_lookup_log::read_records(dir.path());
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].outcome, "miss_no_evidence");
    assert_eq!(records[0].selected, "Quokka Ledger");
    assert_eq!(records[0].engine, vibe_talk::glossary_lookup_log::ENGINE);
    assert!(!records[0].queries.is_empty());
    assert_eq!(records[1].outcome, "error");
    assert_eq!(records[1].error.as_deref(), Some("bad_request"));
    let text = std::fs::read_to_string(&files[0]).expect("log");
    assert!(!text.contains(WRITE_TOKEN), "no token in the log");

    // Switched off: no log directory at all.
    let backend = backend_with(vec![message(1, "nothing relevant here")]);
    let (state, _summarizer, dir) =
        cached_state_with(backend, true, "[summaries]\nwtf_lookup_log = false\n").await;
    fill(&state).await;
    let app = vibe_talk::http::router(state);
    let (status, _) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "Quokka Ledger"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!vibe_talk::glossary_lookup_log::log_dir(dir.path()).exists());
}

#[tokio::test]
async fn a_long_selection_with_no_evidence_stays_inside_the_query_budget() {
    // `#2232`: an eight-word selection that matches nothing runs at most the
    // query budget of all-channel searches, and the not-found sentence names
    // only searches that ran.
    let backend = backend_with(vec![message(1, "nothing relevant here")]);
    let (state, summarizer, dir) = cached_state(backend, true).await;
    fill(&state).await;
    let app = vibe_talk::http::router(state);
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "Zorblat seven, the quuxing frobnitz manifold sweep check"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("not_found"), "{body}");
    assert!(
        summarizer.prompt_calls().is_empty(),
        "no evidence, no model call"
    );
    let records = vibe_talk::glossary_lookup_log::read_records(dir.path());
    let record = &records[0];
    assert_eq!(record.head, "Zorblat seven");
    assert_eq!(
        record.queries.len(),
        vibe_talk::glossary_query::QUERY_BUDGET,
        "{record:?}"
    );
    assert_eq!(record.stopped.as_deref(), Some("query_budget"));
    assert_eq!(record.queries[0].q, "Zorblat seven");
    let reason = body["reason"].as_str().expect("reason");
    assert!(reason.contains("\"Zorblat seven\""), "{reason}");
    assert!(reason.contains("shorter forms"), "{reason}");
    assert!(
        reason.contains("recent messages for \"Zorblat seven\""),
        "{reason}"
    );
    assert!(
        !reason.contains("\"sweep\""),
        "an unrun search is never named: {reason}"
    );
}

#[tokio::test]
async fn a_probe_needs_the_switch_skips_learned_entries_and_never_saves() {
    let defining = message(1, "Row 4: the nightly widget soak, owned by team green");
    let use_site = message(2, "Row 4, the soak, is red again");
    let source_id = defining.id.as_str().to_owned();
    let use_site_id = use_site.id.as_str().to_owned();
    let answer = format!(
        "{{\"term\":\"Row 4\",\"definition\":\"the nightly widget soak\",\"source_kind\":\"message\",\"source_id\":\"{source_id}\"}}"
    );
    let selection = json!({"selected": "Row 4, the soak", "message_id": use_site_id});
    let probe_path = |mode: &str| format!("{}?probe={mode}", lookup_path());

    // Without the switch a probe is refused by name, and nothing is spent.
    let backend = backend_with(vec![defining.clone(), use_site.clone()]);
    let (state, summarizer, _dir) = cached_state(backend, true).await;
    fill(&state).await;
    let app = vibe_talk::http::router(state);
    let (status, body) = post(
        &app,
        &probe_path("retrieval"),
        Some(WRITE_TOKEN),
        &selection,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.to_string().contains("probe_disabled"), "{body}");
    let (status, _) = post(&app, &probe_path("bogus"), Some(WRITE_TOKEN), &selection).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(summarizer.prompt_calls().is_empty());

    // With the switch: a retrieval probe makes no model call and reports
    // what it did; a full probe calls the model but saves nothing; both skip
    // a learned entry for the same words.
    let backend = backend_with(vec![defining, use_site]);
    let (state, summarizer, dir) =
        cached_state_with(backend, true, "[summaries]\nwtf_probe = true\n").await;
    fill(&state).await;
    summarizer.answer_with(&answer);
    let app = vibe_talk::http::router(state);
    let (status, body) = post(&app, &lookup_path(), Some(WRITE_TOKEN), &selection).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["saved"], json!(true), "a reader press saves: {body}");
    assert!(body.get("diagnostics").is_none(), "{body}");
    assert_eq!(summarizer.prompt_calls().len(), 1);
    let (status, body) = post(
        &app,
        &probe_path("retrieval"),
        Some(WRITE_TOKEN),
        &selection,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("evidence"), "{body}");
    assert_eq!(body["term"], json!("Row 4"), "{body}");
    assert_eq!(body["diagnostics"]["outcome"], json!("evidence"), "{body}");
    assert_eq!(body["diagnostics"]["probe"], json!("retrieval"), "{body}");
    assert_eq!(
        body["diagnostics"]["candidates"][0]["bucket"],
        json!("channel_recent"),
        "{body}"
    );
    assert_eq!(
        summarizer.prompt_calls().len(),
        1,
        "a retrieval probe calls no model"
    );
    let (status, body) = post(&app, &probe_path("full"), Some(WRITE_TOKEN), &selection).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("found"), "{body}");
    assert_eq!(
        body["cached"],
        json!(false),
        "the learned entry was skipped: {body}"
    );
    assert_eq!(body["saved"], json!(false), "a probe never saves: {body}");
    assert_eq!(summarizer.prompt_calls().len(), 2);
    let rows = std::fs::read_to_string(vibe_talk::glossary_dynamic::glossary_path(dir.path()))
        .expect("learned file");
    assert_eq!(
        rows.lines().count(),
        1,
        "only the reader press saved: {rows}"
    );
    let records = vibe_talk::glossary_lookup_log::read_records(dir.path());
    let outcomes: Vec<&str> = records.iter().map(|r| r.outcome.as_str()).collect();
    assert_eq!(outcomes, vec!["found", "evidence", "found"]);
}

#[tokio::test]
async fn the_replay_command_runs_logged_misses_against_a_local_server() {
    // `#2232`: the real CLI path over a real loopback socket. The log holds
    // one old miss; replaying it against the fixed engine hits.
    let defining = message(1, "Row 7: widget build on the baseline image");
    let use_site = message(2, "Status: Row 7, the alpha-12-v3 gate test is green");
    let use_site_id = use_site.id.as_str().to_owned();
    let backend = backend_with(vec![defining, use_site]);
    let (state, _summarizer, dir) =
        cached_state_with(backend, true, "[summaries]\nwtf_probe = true\n").await;
    fill(&state).await;
    let app = vibe_talk::http::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let work = testing::TempDir::new("wtf-replay");
    let old_log = work.path().join("old-state");
    let miss = vibe_talk::glossary_lookup_log::LookupRecord {
        v: 1,
        ts: "2026-09-20T10:00:00Z".to_owned(),
        engine: "wtf-engine-1".to_owned(),
        channel: WRITE_CHANNEL.to_owned(),
        message_id: Some(use_site_id),
        selected: "Row 7, the alpha-12-v3 gate test".to_owned(),
        outcome: "miss_no_evidence".to_owned(),
        ..Default::default()
    };
    vibe_talk::glossary_lookup_log::append(
        &vibe_talk::glossary_lookup_log::log_dir(&old_log),
        &miss,
        "2026-09-20T10:00:00Z".parse().expect("ts"),
        vibe_talk::glossary_lookup_log::DAY_CAP_BYTES,
        100_000,
    )
    .expect("seed log");
    let token_file = work.path().join("token");
    std::fs::write(&token_file, format!("{WRITE_TOKEN}\n")).expect("token");
    let out = work.path().join("after.json");
    let server = format!("http://127.0.0.1:{port}");
    let args = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    // A denied port is refused before any request.
    let refused = vibe_talk::wtf_replay::run_cli(args(&[
        "run",
        "--server",
        &server,
        "--token-file",
        token_file.to_str().unwrap(),
        "--log",
        old_log.to_str().unwrap(),
        "--deny-port",
        &port.to_string(),
    ]))
    .await;
    assert!(refused.is_err());
    let printed = vibe_talk::wtf_replay::run_cli(args(&[
        "run",
        "--server",
        &server,
        "--token-file",
        token_file.to_str().unwrap(),
        "--log",
        old_log.to_str().unwrap(),
        "--pace-ms",
        "0",
        "--out",
        out.to_str().unwrap(),
    ]))
    .await
    .expect("replay runs");
    assert!(printed.contains("HIT"), "{printed}");
    assert!(printed.contains("1/1 hits"), "{printed}");
    assert!(!printed.contains(WRITE_TOKEN), "the token is never printed");
    let report: vibe_talk::wtf_replay::RunReport =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("report")).expect("parse");
    assert_eq!(report.results[0].outcome.as_deref(), Some("evidence"));
    assert_eq!(
        report.engine.as_deref(),
        Some(vibe_talk::glossary_lookup_log::ENGINE)
    );
    // The server logged the probe as a probe, so a later replay skips it.
    let records = vibe_talk::glossary_lookup_log::read_records(dir.path());
    assert_eq!(
        records.last().and_then(|r| r.probe.as_deref()),
        Some("retrieval")
    );
    let compared = vibe_talk::wtf_replay::run_cli(args(&[
        "compare",
        out.to_str().unwrap(),
        out.to_str().unwrap(),
    ]))
    .await
    .expect("compare");
    assert!(compared.contains("gained (0)"), "{compared}");
}

/// `#2233 vibe-talk-glossary-popup`: the full-screen glossary's exact request, `GET
/// /api/v1/glossary/terms` with the page's token and no query, through the real router: the reviewed
/// entries of the warm-up glossary and the learned ones in the deployment's store, each once, with
/// the fields the popup draws (term, definition, source, origin). A learned definition comes back
/// exactly as stored, markup and all: the popup draws it as text, and nothing on the way renders it.
/// Every term here is synthetic.
#[tokio::test]
async fn the_glossary_popup_reads_every_term_with_its_origin() {
    let defining = message(
        1,
        "The Zephyr cache keeps runner results for the synthetic fleet",
    );
    let source_id = defining.id.as_str().to_owned();
    let backend = backend_with(vec![
        defining,
        message(2, "the Zephyr cache warmed up again"),
    ]);
    let dir = testing::TempDir::new("glossary-popup");
    let reviewed = dir.path().join("glossary.md");
    std::fs::write(
        &reviewed,
        "# Synthetic glossary\n\n- **alpha quorum**: three made-up votes out of five\n- **Runner**: a synthetic build worker\n",
    )
    .expect("reviewed glossary");
    let store_path = dir.path().join("store.db");
    let text = format!(
        "{}\n[storage]\npath = \"{}\"\n[summaries]\ncatchup_warmup_files = [\"{}\"]\n",
        testing::config_toml(),
        store_path.display(),
        reviewed.display()
    );
    let (mut state, _discord, _store, summarizer) = testing::state_with_summarizer(&text);
    state.replace_chat(backend.clone());
    let clock = testing::ManualClock::at(base_ms() + 3_600_000);
    let mut state = testing::with_message_cache(state, dir.path(), clock).await;
    state.message_cache.set_serve(true);
    state.catchup_summarizer = Some(summarizer.clone());
    state.catchup_model = Some(Arc::from("fake-model"));
    fill(&state).await;
    summarizer.answer_with(&format!(
        "{{\"term\":\"Zephyr cache\",\"definition\":\"<img src=x onerror=1> a made-up cache for runner results\",\"source_kind\":\"message\",\"source_id\":\"{source_id}\"}}"
    ));
    let app = vibe_talk::http::router(state);
    let (status, body) = post(
        &app,
        &lookup_path(),
        Some(WRITE_TOKEN),
        &json!({"selected": "Zephyr cache"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("found"), "{body}");
    let reads = backend.reads();
    // The popup's own request: a read token, as a read-only page holds.
    let (status, body) = get(&app, "/api/v1/glossary/terms", READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        backend.reads(),
        reads,
        "the term list must not call upstream"
    );
    let terms = body["terms"].as_array().expect("terms");
    let named = |term: &str| {
        terms
            .iter()
            .find(|t| t["term"] == json!(term))
            .unwrap_or_else(|| panic!("no {term} in {body}"))
            .clone()
    };
    let quorum = named("alpha quorum");
    assert_eq!(quorum["origin"], json!("reviewed"), "{body}");
    assert_eq!(
        quorum["definition"],
        json!("three made-up votes out of five"),
        "{body}"
    );
    assert_eq!(quorum["source"], Value::Null, "{body}");
    assert_eq!(named("Runner")["origin"], json!("reviewed"), "{body}");
    let learned = named("Zephyr cache");
    assert_eq!(learned["origin"], json!("dynamic"), "{body}");
    assert_eq!(
        learned["definition"],
        json!("<img src=x onerror=1> a made-up cache for runner results"),
        "a learned definition must come back as stored, for the popup to draw as text: {body}"
    );
    assert_eq!(learned["source"]["kind"], json!("message"), "{body}");
    let href = learned["source"]["href"].as_str().expect("href");
    assert!(
        href.starts_with('?') && href.contains("&m="),
        "a message source is the in-app address the popup links: {href}"
    );
    // Each term once.
    let mut seen: Vec<String> = terms
        .iter()
        .map(|t| t["term"].as_str().unwrap_or_default().to_lowercase())
        .collect();
    let all = seen.len();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), all, "a term is served twice: {body}");
}
