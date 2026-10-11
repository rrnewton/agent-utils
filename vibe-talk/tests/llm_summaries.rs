//! `#233 llm-summarizer` and `#182 thread-display-names`, through the HTTP routes.
//!
//! The Messages API is a LOCAL mock started by each test; nothing here reaches a model provider.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode, Uri};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::state::AppState;
use vibe_talk::summarize::fake::FakeSummarizer;
use vibe_talk::summarize::threads::{record_key, ThreadNames, ThreadRecord};
use vibe_talk::testing::{self, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::threads::{ThreadSummary, TimelinePage, TimelineRequest, TimelineView};

// --- a local Messages API -----------------------------------------------------------------------

#[derive(Clone, Default)]
struct ModelMock {
    seen: Arc<Mutex<Vec<(String, HeaderMap, Value)>>>,
}

async fn answer(
    State(mock): State<ModelMock>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> String {
    mock.seen.lock().unwrap().push((
        uri.to_string(),
        headers,
        serde_json::from_slice(&body).unwrap_or(Value::Null),
    ));
    json!({
        "content": [{"type": "text", "text": "The cache key now folds in the feature set, and the reviewer is asked about disk use."}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 300, "output_tokens": 20}
    })
    .to_string()
}

async fn model_mock() -> (ModelMock, String) {
    let mock = ModelMock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .fallback(answer)
        .with_state(mock.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serves") });
    (mock, base)
}

async fn call(app: &axum::Router, path: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_configured_llm_section_is_the_summariser_the_summary_route_uses() {
    let (mock, base) = model_mock().await;
    let dir = testing::TempDir::new("llm-route");
    let key = dir.path().join("model-key");
    std::fs::write(&key, "sk-route-test-key\n").unwrap();
    let text = format!(
        "{}\n[summaries.llm]\ndialect = \"anthropic\"\nbase_url = \"{base}\"\napi_key_file = \"{}\"\n",
        testing::config_toml_without_elevenlabs(),
        key.display()
    );
    let config = vibe_talk::config::Config::from_toml_and_env(&text, &Default::default())
        .expect("valid configuration");
    // Built the way `main` builds it, so this is the selection the binary makes.
    let summarizer = vibe_talk::summarize::summarizer_for(
        &config.elevenlabs,
        &config.summaries,
        Arc::new(vibe_talk::elevenlabs::fake::FakeElevenLabs::new()),
    );
    assert_eq!(
        summarizer.backend(),
        vibe_talk::summarize::messages_api::BACKEND
    );
    assert_eq!(
        summarizer.unavailable_reason(),
        None,
        "no ElevenLabs credentials, and that is fine: they are not what summarises"
    );
    let (state, discord, _store) = testing::state_with_backend(&text, summarizer);
    assert!(
        state.summary_version.starts_with("v1-messages-api-"),
        "{}",
        state.summary_version
    );
    let id = discord
        .seed(
            &ChannelId(WRITE_CHANNEL.to_owned()),
            "codex-eng",
            &"The cache key used to be the source path plus the compiler version. ".repeat(10),
        )
        .0;
    let app = vibe_talk::http::router(state);

    let (status, body) = call(
        &app,
        &format!("/api/v1/channels/{WRITE_CHANNEL}/messages/{id}/summary"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "generated", "{body}");
    assert!(
        body["summary"]
            .as_str()
            .unwrap_or_default()
            .starts_with("The cache key now folds"),
        "{body}"
    );
    assert!(
        body["backend"].as_str().unwrap().contains("no tools"),
        "{body}"
    );

    let seen = mock.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "one summary, one call");
    let (path, headers, sent) = &seen[0];
    assert_eq!(path, "/v1/messages");
    assert_eq!(headers["x-api-key"], "sk-route-test-key");
    assert!(
        sent.get("tools").is_none() && sent.get("tool_choice").is_none(),
        "{sent}"
    );

    // The second ask is a cache hit: the model is not asked again.
    let (_, again) = call(
        &app,
        &format!("/api/v1/channels/{WRITE_CHANNEL}/messages/{id}/summary"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(again["state"], "cached", "{again}");
    assert_eq!(mock.seen.lock().unwrap().len(), 1);
}

// --- thread names -------------------------------------------------------------------------------

/// A provider with two threads whose reply counts a test can move.
#[derive(Default)]
struct Threads {
    replies: Mutex<[u64; 2]>,
    thread_reads: Mutex<Vec<String>>,
}

const THREADS: [&str; 2] = ["spaces/A/threads/runner", "spaces/A/threads/cache"];

fn message(id: &str, content: &str) -> Message {
    Message {
        thread: None,
        id: MessageId(id.to_owned()),
        channel_id: ChannelId(WRITE_CHANNEL.to_owned()),
        author: "claude-integ".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: "2026-10-11T08:00:00Z".to_owned(),
        spoken_time: String::new(),
        reply_to: None,
        content: content.to_owned(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        reactions: None,
    }
}

impl Threads {
    fn summary(&self, n: usize) -> ThreadSummary {
        ThreadSummary {
            id: THREADS[n].to_owned(),
            root: Some(message(
                &format!("root-{n}"),
                &format!("thread {n} starts here"),
            )),
            title: format!("thread {n} starts here"),
            reply_count: Some(self.replies.lock().unwrap()[n]),
            reply_count_exact: true,
            updated_at: format!("2026-10-11T08:0{n}:00Z"),
            display_name: None,
            summary: None,
        }
    }

    fn grow(&self, n: usize, to: u64) {
        self.replies.lock().unwrap()[n] = to;
    }
}

#[async_trait::async_trait]
impl ChatClient for Threads {
    fn provider_name(&self) -> &str {
        "Google Chat"
    }
    fn supports_threading(&self) -> bool {
        true
    }
    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        Ok(ChatIdentity {
            id: "7".to_owned(),
            username: "Reader".to_owned(),
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
        match request.view {
            TimelineView::Threads => Ok(TimelinePage {
                threads: vec![self.summary(0), self.summary(1)],
                has_threads: true,
                ..TimelinePage::default()
            }),
            TimelineView::Thread => {
                let id = request.thread_id.clone().unwrap_or_default();
                self.thread_reads.lock().unwrap().push(id.clone());
                let n = THREADS
                    .iter()
                    .position(|t| *t == id)
                    .expect("a known thread");
                let summary = self.summary(n);
                let mut messages = vec![summary.root.clone().unwrap()];
                for reply in 0..summary.reply_count.unwrap_or(0) {
                    messages.push(message(
                        &format!("reply-{n}-{reply}"),
                        &format!("reply {reply}"),
                    ));
                }
                Ok(TimelinePage {
                    messages,
                    thread: Some(summary),
                    has_threads: true,
                    ..TimelinePage::default()
                })
            }
            _ => Ok(TimelinePage::default()),
        }
    }
    async fn post_message(
        &self,
        _channel: &ChannelId,
        _content: &str,
        _reply: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused("read only".to_owned()))
    }
}

struct Harness {
    app: axum::Router,
    state: AppState,
    threads: Arc<Threads>,
    summarizer: Arc<FakeSummarizer>,
    names: Arc<ThreadNames>,
}

fn harness() -> Harness {
    let summarizer = Arc::new(FakeSummarizer::naming_threads());
    let (mut state, _discord, _store) =
        testing::state_with_backend(&testing::config_toml(), summarizer.clone());
    let threads = Arc::new(Threads::default());
    state.replace_chat(threads.clone());
    Harness {
        app: vibe_talk::http::router(state.clone()),
        names: Arc::clone(&state.thread_names),
        state,
        threads,
        summarizer,
    }
}

const THREAD_LIST: &str = "/api/v1/channels/2222222222/timeline?view=threads";

async fn until_named(names: &ThreadNames, count: usize) {
    for _ in 0..500 {
        if names.named() >= count && names.pending() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "{count} namings never completed: {} done, {} pending",
        names.named(),
        names.pending()
    );
}

fn names_of(body: &Value) -> Vec<(String, Option<String>, Option<String>)> {
    body["threads"]
        .as_array()
        .expect("threads")
        .iter()
        .map(|thread| {
            (
                thread["id"].as_str().unwrap().to_owned(),
                thread["display_name"].as_str().map(str::to_owned),
                thread["summary"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_thread_list_read_by_the_owner_names_its_threads_in_the_background() {
    let h = harness();
    let (status, first) = call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    // Never on the read path: the first answer is not held up by the model.
    for (_, name, _) in names_of(&first) {
        assert_eq!(name, None, "the first read waited for a name: {first}");
    }
    until_named(&h.names, 2).await;

    let (_, second) = call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    let named = names_of(&second);
    let mut slugs: Vec<_> = named
        .iter()
        .map(|(_, name, _)| name.clone().expect("named"))
        .collect();
    slugs.sort();
    assert_eq!(slugs, ["thread-1", "thread-2"]);
    assert!(
        named.iter().all(|(_, _, sentence)| sentence.is_some()),
        "{second}"
    );
    assert_eq!(
        h.summarizer.thread_prompts().len(),
        2,
        "one call per thread"
    );
    // Each prompt is the thread's own text, fenced.
    for prompt in h.summarizer.thread_prompts() {
        assert!(prompt.contains(vibe_talk::untrusted::FENCE), "{prompt}");
        assert!(prompt.contains("starts here"), "{prompt}");
    }

    // ...and the record is filed, under the summary policy version.
    let filed = h
        .state
        .store
        .cached_summary(&record_key(
            &h.state.summary_version,
            &ChannelId(WRITE_CHANNEL.to_owned()),
            THREADS[0],
        ))
        .await
        .expect("readable")
        .expect("filed");
    let record: ThreadRecord = serde_json::from_str(&filed).expect("a record");
    assert_eq!(record.at, 1, "named at one message: the root");

    // A read with nothing due asks nothing.
    let (_, _) = call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.summarizer.thread_prompts().len(), 2, "nothing was due");
}

#[tokio::test]
async fn the_name_is_kept_as_the_thread_grows_and_only_the_sentence_follows_the_schedule() {
    let h = harness();
    call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    until_named(&h.names, 2).await;
    let (_, before) = call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    let (_, name_before, sentence_before) = names_of(&before)
        .into_iter()
        .find(|(id, _, _)| id == THREADS[0])
        .unwrap();

    // One reply: two messages, which is due (1, 2, 4, 8, then every 10).
    h.threads.grow(0, 1);
    call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    until_named(&h.names, 3).await;
    let (_, after) = call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    let (_, name_after, sentence_after) = names_of(&after)
        .into_iter()
        .find(|(id, _, _)| id == THREADS[0])
        .unwrap();
    assert_eq!(
        name_after, name_before,
        "a display name is chosen once and kept"
    );
    assert_ne!(sentence_after, sentence_before, "the sentence is refreshed");

    // Two replies is three messages: not due until four.
    h.threads.grow(0, 2);
    call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.names.named(), 3, "three messages is not on the schedule");
}

#[tokio::test]
async fn a_read_token_is_shown_names_but_never_spends_the_model() {
    let h = harness();
    let (status, body) = call(&h.app, THREAD_LIST, READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.names.pending() + h.names.named(), 0);
    assert!(h.threads.thread_reads.lock().unwrap().is_empty());
    assert!(h.summarizer.thread_prompts().is_empty());

    // Once the owner's read has named them, a read token is served the names.
    call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    until_named(&h.names, 2).await;
    let (_, body) = call(&h.app, THREAD_LIST, READ_TOKEN).await;
    assert!(
        names_of(&body).iter().all(|(_, name, _)| name.is_some()),
        "{body}"
    );
}

#[tokio::test]
async fn a_forward_delta_carries_a_thread_renamed_since_the_page_was_read() {
    let h = harness();
    let (_, first) = call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    let after = first["next_after"]
        .as_str()
        .expect("a forward cursor")
        .to_owned();
    until_named(&h.names, 2).await;

    // Nothing changed upstream, so without the renamed threads this delta would be empty and a page
    // that refreshes by delta would never learn the names.
    let (status, delta) = call(
        &h.app,
        &format!("{THREAD_LIST}&after={}", urlencode(&after)),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{delta}");
    assert!(delta["delta"].is_object(), "this is a delta: {delta}");
    let named = names_of(&delta);
    assert_eq!(named.len(), 2, "{delta}");
    assert!(named.iter().all(|(_, name, _)| name.is_some()), "{delta}");
}

#[tokio::test]
async fn names_survive_a_restart_through_the_store() {
    let h = harness();
    call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    until_named(&h.names, 2).await;

    // A restart: the same store, nothing in memory.
    let restarted = AppState {
        thread_names: Arc::new(ThreadNames::new()),
        ..h.state.clone()
    };
    let names = Arc::clone(&restarted.thread_names);
    let app = vibe_talk::http::router(restarted);
    let (_, body) = call(&app, THREAD_LIST, WRITE_TOKEN).await;
    assert!(
        names_of(&body).iter().all(|(_, name, _)| name.is_some()),
        "the names were not read back: {body}"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(names.named(), 0, "nothing was renamed after the restart");
    assert_eq!(h.summarizer.thread_prompts().len(), 2);
}

#[tokio::test]
async fn a_summariser_that_cannot_name_threads_leaves_the_list_as_it_was() {
    let (mut state, _discord) = testing::state();
    let threads = Arc::new(Threads::default());
    state.replace_chat(threads.clone());
    let names = Arc::clone(&state.thread_names);
    let app = vibe_talk::http::router(state);
    let (status, body) = call(&app, THREAD_LIST, WRITE_TOKEN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(names.pending() + names.named(), 0);
    assert!(threads.thread_reads.lock().unwrap().is_empty());
    assert!(names_of(&body).iter().all(|(_, name, _)| name.is_none()));
}

fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
