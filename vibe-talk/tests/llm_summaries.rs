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
use vibe_talk::store::fake::FakeStore;
use vibe_talk::store::{StateStore as _, SummaryKey};
use vibe_talk::summarize::fake::FakeSummarizer;
use vibe_talk::summarize::threads::{
    name_key, sentence_key, FiledName, FiledSentence, ThreadDeps, ThreadNames,
};
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

#[tokio::test]
async fn a_host_overlay_moving_to_vertex_sends_none_of_the_shared_files_key() {
    // The two-host case end to end: the SHARED file says anthropic with a real key file, and this
    // host's overlay moves to a Vertex gateway (here, the local mock) with its own token. Nothing
    // of the shared section may reach the gateway, the Anthropic key above all.
    let (mock, base) = model_mock().await;
    let dir = testing::TempDir::new("llm-overlay-vertex");
    let shared_key = dir.path().join("anthropic-key");
    std::fs::write(&shared_key, "sk-shared-anthropic-key\n").unwrap();
    let token = dir.path().join("vertex-token");
    std::fs::write(&token, "vertex-bearer-token\n").unwrap();
    let shared = dir.path().join("shared.toml");
    std::fs::write(
        &shared,
        format!(
            "{}\n[summaries.llm]\ndialect = \"anthropic\"\napi_key_file = \"{}\"\n",
            testing::config_toml_without_elevenlabs(),
            shared_key.display()
        ),
    )
    .unwrap();
    let overlay = dir.path().join("this-host.toml");
    std::fs::write(
        &overlay,
        format!(
            "[summaries.llm]\ndialect = \"vertex\"\nbase_url = \"{base}\"\nproject = \"my-project\"\n\
             region = \"global\"\nbearer_token_file = \"{}\"\n",
            token.display()
        ),
    )
    .unwrap();
    let config =
        vibe_talk::config::Config::load_layered(Some(&shared), std::slice::from_ref(&overlay))
            .expect("valid layered configuration");
    let llm = config.summaries.llm.clone().expect("the overlay's section");
    assert_eq!(llm.api_key_file, None, "the shared key file was inherited");
    let summarizer = vibe_talk::summarize::summarizer_for(
        &config.elevenlabs,
        &config.summaries,
        Arc::new(vibe_talk::elevenlabs::fake::FakeElevenLabs::new()),
    );
    let target = message("1", &"A long message about the runner pool. ".repeat(20));
    summarizer
        .summarize(&vibe_talk::summarize::SummaryRequest::new(
            &target,
            &[],
            160,
        ))
        .await
        .expect("the gateway answers");

    let seen = mock.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let (path, headers, _) = &seen[0];
    assert!(
        path.ends_with("/projects/my-project/locations/global/publishers/anthropic/models/claude-haiku-4-5:rawPredict"),
        "{path}"
    );
    assert!(
        headers.get("x-api-key").is_none(),
        "the shared file's Anthropic key went to the vertex gateway: {headers:?}"
    );
    assert_eq!(headers["authorization"], "Bearer vertex-bearer-token");
    for value in headers.values() {
        assert!(
            !value.to_str().unwrap_or_default().contains("sk-shared"),
            "{headers:?}"
        );
    }
}

// --- thread names -------------------------------------------------------------------------------

/// A provider with two threads whose reply counts a test can move.
#[derive(Default)]
struct Threads {
    replies: Mutex<[u64; 2]>,
    thread_reads: Mutex<Vec<String>>,
    /// When set, every thread's `updated_at`.
    updated: Mutex<Option<String>>,
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
        heading: None,
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
            updated_at: self
                .updated
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| format!("2026-10-11T08:0{n}:00Z")),
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
    store: Arc<FakeStore>,
}

fn harness() -> Harness {
    let summarizer = Arc::new(FakeSummarizer::naming_threads());
    let (mut state, _discord, store) =
        testing::state_with_backend(&testing::config_toml(), summarizer.clone());
    let threads = Arc::new(Threads::default());
    state.replace_chat(threads.clone());
    Harness {
        app: vibe_talk::http::router(state.clone()),
        names: Arc::clone(&state.thread_names),
        state,
        threads,
        summarizer,
        store,
    }
}

fn channel() -> ChannelId {
    ChannelId("2222222222".to_owned())
}

/// The thread list as the provider answers it, before any name is attached.
async fn thread_page(threads: &Threads) -> (TimelineRequest, TimelinePage) {
    let request = TimelineRequest {
        view: TimelineView::Threads,
        thread_id: None,
        before: None,
        after: None,
        limit: 50,
    };
    let page = threads
        .fetch_timeline(&channel(), &request)
        .await
        .expect("a page");
    (request, page)
}

/// Each thread's id and display name, from an annotated page.
fn page_names(page: &TimelinePage) -> Vec<(String, Option<String>)> {
    page.threads
        .iter()
        .map(|thread| (thread.id.clone(), thread.display_name.clone()))
        .collect()
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

    // ...and both halves are filed: the name under the version no policy change removes, the
    // sentence under the summary policy version.
    let name = h
        .state
        .store
        .cached_summary(&name_key(&channel(), THREADS[0]))
        .await
        .expect("readable")
        .expect("filed");
    let name: FiledName = serde_json::from_str(&name).expect("a name row");
    assert!(name.name.starts_with("thread-"), "{name:?}");
    let sentence = h
        .state
        .store
        .cached_summary(&sentence_key(
            &h.state.summary_version,
            &channel(),
            THREADS[0],
        ))
        .await
        .expect("readable")
        .expect("filed");
    let sentence: FiledSentence = serde_json::from_str(&sentence).expect("a sentence row");
    assert_eq!(sentence.at, 1, "named at one message: the root");

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
async fn a_name_survives_a_policy_change_and_a_flood_of_newer_summaries() {
    // The display name is chosen ONCE. Filed under the summary policy version, it used to be
    // swept by any policy change and evicted by ordinary summary traffic, and the thread was then
    // given a different name.
    let h = harness();
    call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    until_named(&h.names, 2).await;
    let (_, before) = call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    let names_before: Vec<_> = names_of(&before)
        .into_iter()
        .map(|(id, name, _)| (id, name))
        .collect();
    assert!(names_before.iter().all(|(_, name)| name.is_some()));

    // A restart under a CHANGED policy: the startup sweep runs, as `main` runs it...
    let version: Arc<str> = Arc::from("v1-messages-api-w3-c160-0000000000000001");
    assert_ne!(version, h.state.summary_version);
    h.state
        .store
        .forget_summaries_except(&version)
        .await
        .expect("the sweep");
    // ...and then more ordinary summaries than the cache's whole ceiling.
    let ceiling = vibe_talk::store::Retention::default().max_summaries;
    for n in 0..=u64::from(ceiling) {
        h.state
            .store
            .cache_summary(
                &SummaryKey {
                    channel: channel(),
                    message: MessageId(format!("1{n:018}")),
                    content_hash: n,
                    version: version.to_string(),
                },
                "an ordinary summary",
            )
            .await
            .expect("cache");
    }
    let restarted = AppState {
        thread_names: Arc::new(ThreadNames::new()),
        summary_version: Arc::clone(&version),
        ..h.state.clone()
    };
    let names = Arc::clone(&restarted.thread_names);
    let app = vibe_talk::http::router(restarted);
    let (_, after) = call(&app, THREAD_LIST, WRITE_TOKEN).await;
    let names_after: Vec<_> = names_of(&after)
        .into_iter()
        .map(|(id, name, _)| (id, name))
        .collect();
    assert_eq!(
        names_after, names_before,
        "a policy change and {ceiling} newer summaries renamed a thread"
    );

    // The SENTENCES belonged to the old policy, so they are asked for again; the names stay.
    until_named(&names, 2).await;
    let (_, later) = call(&app, THREAD_LIST, WRITE_TOKEN).await;
    let later = names_of(&later);
    assert!(later.iter().all(|(_, _, sentence)| sentence.is_some()));
    assert_eq!(
        later
            .into_iter()
            .map(|(id, name, _)| (id, name))
            .collect::<Vec<_>>(),
        names_before
    );
}

#[tokio::test]
async fn a_store_that_cannot_be_read_never_gives_a_thread_a_second_name() {
    let h = harness();
    call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    until_named(&h.names, 2).await;
    let deps = ThreadDeps::from_state(&h.state);
    let (request, mut page) = thread_page(&h.threads).await;
    h.names
        .annotate(&deps, &channel(), &request, &mut page, true)
        .await;
    let names_before = page_names(&page);

    // A restart, so nothing is in memory, and the store's next read fails.
    let names = Arc::new(ThreadNames::new());
    h.store.fail_next("the disk is busy");
    let (request, mut page) = thread_page(&h.threads).await;
    names
        .annotate(&deps, &channel(), &request, &mut page, true)
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        page_names(&page)[0].1,
        None,
        "the name that could not be read is not invented"
    );
    assert_eq!(
        names.pending() + names.named(),
        0,
        "a thread whose name could not be READ was queued for a new one"
    );
    assert_eq!(
        h.summarizer.thread_prompts().len(),
        2,
        "the model was asked again"
    );

    // Once the store answers, the name it held is the name shown.
    let (request, mut page) = thread_page(&h.threads).await;
    names
        .annotate(&deps, &channel(), &request, &mut page, true)
        .await;
    assert_eq!(page_names(&page), names_before);
}

#[tokio::test]
async fn a_naming_queued_on_stale_knowledge_keeps_the_name_already_filed() {
    // One instance learns, through a read token, that the threads have no name, and remembers it.
    // Another then names them. When the first is finally read by the owner, its memory still says
    // "unnamed" and it queues both — and the naming must re-read the store and keep the names.
    let h = harness();
    let deps = ThreadDeps::from_state(&h.state);
    let stale = Arc::new(ThreadNames::new());
    let (request, mut page) = thread_page(&h.threads).await;
    stale
        .annotate(&deps, &channel(), &request, &mut page, false)
        .await;
    assert!(page_names(&page).iter().all(|(_, name)| name.is_none()));

    call(&h.app, THREAD_LIST, WRITE_TOKEN).await;
    until_named(&h.names, 2).await;
    let (request, mut page) = thread_page(&h.threads).await;
    h.names
        .annotate(&deps, &channel(), &request, &mut page, false)
        .await;
    let filed = page_names(&page);

    let (request, mut page) = thread_page(&h.threads).await;
    stale
        .annotate(&deps, &channel(), &request, &mut page, true)
        .await;
    until_named(&stale, 2).await;
    let (request, mut page) = thread_page(&h.threads).await;
    stale
        .annotate(&deps, &channel(), &request, &mut page, false)
        .await;
    assert_eq!(page_names(&page), filed, "a filed name was replaced");
}

#[tokio::test]
async fn an_active_thread_keeps_its_name_row_fresh_without_asking_the_model() {
    // The age limit counts from a row's last write. A thread that stays active but between
    // sentence refreshes must not age its name out, so a read that sees it active rewrites the
    // row, at most daily, with no model call.
    let h = harness();
    let deps = ThreadDeps::from_state(&h.state);
    let now = jiff::Timestamp::now().as_second();
    let old = now - 2 * 24 * 60 * 60;
    *h.threads.updated.lock().unwrap() = Some(
        jiff::Timestamp::from_second(now - 3_600)
            .unwrap()
            .to_string(),
    );
    for thread in THREADS {
        h.store
            .cache_summary(
                &name_key(&channel(), thread),
                &serde_json::to_string(&FiledName {
                    name: "runner-wedged".to_owned(),
                    touched: old,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        h.store
            .cache_summary(
                &sentence_key(&h.state.summary_version, &channel(), thread),
                &serde_json::to_string(&FiledSentence {
                    summary: "It is stuck.".to_owned(),
                    at: 1,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let (request, mut page) = thread_page(&h.threads).await;
    h.names
        .annotate(&deps, &channel(), &request, &mut page, true)
        .await;
    let touched = || async {
        let text = h
            .store
            .cached_summary(&name_key(&channel(), THREADS[0]))
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str::<FiledName>(&text).unwrap()
    };
    for _ in 0..100 {
        if touched().await.touched > old {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let row = touched().await;
    assert!(row.touched >= now, "the row was not rewritten: {row:?}");
    assert_eq!(row.name, "runner-wedged", "the name itself is unchanged");
    assert!(
        h.summarizer.thread_prompts().is_empty(),
        "keeping a name fresh asked the model"
    );
    assert_eq!(h.names.pending() + h.names.named(), 0);
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

#[tokio::test]
async fn diagnostics_judge_the_llm_section_and_not_the_absent_elevenlabs_credentials() {
    let dir = testing::TempDir::new("llm-diagnostics");
    let key = dir.path().join("model-key");
    let text = format!(
        "{}\n[summaries.llm]\napi_key_file = \"{}\"\n",
        testing::config_toml_without_elevenlabs(),
        key.display()
    );
    let config = vibe_talk::config::Config::from_toml_and_env(&text, &Default::default())
        .expect("valid configuration");
    let summarizer = vibe_talk::summarize::summarizer_for(
        &config.elevenlabs,
        &config.summaries,
        Arc::new(vibe_talk::elevenlabs::fake::FakeElevenLabs::new()),
    );
    let (state, _discord, _store) = testing::state_with_backend(&text, summarizer);
    let row = |report: &vibe_talk::diagnostics::DiagnosticsReport| {
        report
            .checks
            .iter()
            .find(|check| check.id == "summaries")
            .cloned()
            .expect("a summaries row")
    };

    // The key file is not there yet: red, unconfigured, and naming the setting.
    let missing = row(&vibe_talk::diagnostics::run(&state).await);
    assert_eq!(
        missing.status,
        vibe_talk::diagnostics::Status::Fail,
        "{missing:#?}"
    );
    assert!(missing.unconfigured, "{missing:#?}");
    assert!(
        missing
            .remedy
            .as_deref()
            .is_some_and(|remedy| remedy.contains("summaries.llm.api_key_file")),
        "{missing:#?}"
    );

    // With it in place: green, saying which dialect and model, though ElevenLabs has no key.
    std::fs::write(&key, "sk-diagnostics-test-key").unwrap();
    let present = row(&vibe_talk::diagnostics::run(&state).await);
    assert_eq!(
        present.status,
        vibe_talk::diagnostics::Status::Pass,
        "{present:#?}"
    );
    let said = format!("{present:?}");
    assert!(
        said.contains("anthropic") && said.contains("claude-haiku-4-5"),
        "{said}"
    );
    assert!(!said.contains("sk-diagnostics-test-key"), "{said}");
}

/// Every `[summaries.llm]` example an operator might copy, from the README's Summaries section and
/// from the shipped example file, uncommented.
fn documented_examples() -> Vec<String> {
    let mut examples = Vec::new();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = std::fs::read_to_string(root.join("README.md")).expect("the README");
    let section = readme
        .split("\n## Summaries\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the README has a Summaries section");
    let mut fenced = section.split("```toml\n").skip(1);
    for block in fenced.by_ref() {
        examples.push(block.split("```").next().unwrap_or_default().to_owned());
    }
    let example =
        std::fs::read_to_string(root.join("vibe-talk.example.toml")).expect("the example file");
    let mut current: Option<String> = None;
    for line in example.lines() {
        let uncommented = line.strip_prefix("# ").or_else(|| line.strip_prefix('#'));
        match (&mut current, uncommented) {
            (None, Some(text)) if text.trim() == "[summaries.llm]" => {
                current = Some(format!("{text}\n"));
            }
            (Some(block), Some(text)) if !text.trim().is_empty() => {
                if text.starts_with('[') || text.contains(" = ") {
                    block.push_str(text);
                    block.push('\n');
                }
            }
            (Some(_), _) => examples.extend(current.take()),
            _ => {}
        }
    }
    examples.extend(current);
    examples
}

#[test]
fn every_documented_example_is_one_this_server_would_accept() {
    let examples = documented_examples();
    assert!(
        examples.len() >= 6,
        "the README and the example file document at least six sections; found {}: {examples:#?}",
        examples.len()
    );
    for example in examples {
        let text = format!("{}\n{example}", testing::config_toml_without_elevenlabs());
        let cfg = vibe_talk::config::Config::from_toml_and_env(&text, &Default::default())
            .unwrap_or_else(|error| panic!("{error}\n{example}"));
        assert!(cfg.summaries.llm.is_some(), "{example}");
    }
}
