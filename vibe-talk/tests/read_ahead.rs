//! `#191 read-aloud-first-lag`: preparing speech reads what the page has just fetched, fetches only
//! what is missing (one shared window fetch, or thread messages a few at a time), and never waits
//! for the provider warm-up.
//!
//! The content-free timing line these requests log is checked in `tests/logging.rs`, where every
//! test captures logs.
//!
//! Every test runs on tokio's paused clock against a chat backend that takes a fixed time per
//! fetch. So "the tap waited for one fetch, not two" is a measured virtual duration, not a race
//! against the machine's real speed.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::read_ahead::THREAD_FETCH_CONCURRENCY;
use vibe_talk::speech::{Description, Playback, Speech, SpeechError, SpeechProvider};
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, READ_CHANNEL, READ_TOKEN};
use vibe_talk::threads::{MessageThread, TimelinePage, TimelineRequest};

const THREAD: &str = "opaque/thread-T";
/// What one chat fetch costs, like the deployed adapter's ~2 s window read, scaled down.
const FETCH: Duration = Duration::from_millis(1_000);
/// What one thread message lookup costs.
const LOOKUP: Duration = Duration::from_millis(100);

fn message(id: &str, thread: Option<&str>) -> Message {
    Message {
        id: MessageId(id.to_owned()),
        channel_id: ChannelId(READ_CHANNEL.to_owned()),
        author: "Reader".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: "2026-10-12T01:00:00Z".to_owned(),
        spoken_time: String::new(),
        reply_to: None,
        content: format!("words of message {id}"),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        thread: thread.map(|thread| MessageThread {
            id: thread.to_owned(),
            root_message_id: None,
            is_root: false,
            reply_count: None,
            reply_count_exact: false,
        }),
        reactions: None,
        heading: None,
    }
}

/// A chat backend that takes [`FETCH`] per window and [`LOOKUP`] per thread message, and counts.
#[derive(Default)]
struct SlowChat {
    windows: AtomicUsize,
    timelines: AtomicUsize,
    lookups: AtomicUsize,
    lookups_in_flight: AtomicUsize,
    most_lookups_in_flight: AtomicUsize,
    looked_up: Mutex<Vec<String>>,
}

impl SlowChat {
    fn window() -> Vec<Message> {
        ["101", "102", "103"]
            .iter()
            .map(|id| message(id, None))
            .collect()
    }

    fn thread() -> Vec<Message> {
        (201..=210)
            .map(|n| message(&n.to_string(), Some(THREAD)))
            .collect()
    }
}

#[async_trait::async_trait]
impl ChatClient for SlowChat {
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
        self.windows.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(FETCH).await;
        Ok(Self::window())
    }

    async fn fetch_timeline(
        &self,
        _channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        self.timelines.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(FETCH).await;
        let messages = match request.thread_id.as_deref() {
            Some(THREAD) => Self::thread(),
            Some(_) => {
                return Err(ChatError::Refused(
                    "thread is outside this channel".to_owned(),
                ))
            }
            None => Self::window(),
        };
        Ok(TimelinePage {
            messages,
            ..TimelinePage::default()
        })
    }

    async fn fetch_thread_message(
        &self,
        _channel: &ChannelId,
        thread: &str,
        id: &MessageId,
    ) -> Result<Message, ChatError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        self.looked_up.lock().unwrap().push(id.0.clone());
        if thread != THREAD {
            return Err(ChatError::Refused(
                "thread is outside this channel".to_owned(),
            ));
        }
        let now = self.lookups_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_lookups_in_flight.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(LOOKUP).await;
        self.lookups_in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(message(&id.0, Some(THREAD)))
    }

    async fn post_message(
        &self,
        _channel: &ChannelId,
        _content: &str,
        _reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused("this test posts nothing".to_owned()))
    }
}

/// A server voice whose warm-up takes `warm_up` and counts how often it was started and finished.
struct TimedSpeech {
    warm_up: Duration,
    started: AtomicUsize,
    finished: AtomicUsize,
}

impl TimedSpeech {
    fn new(warm_up: Duration) -> Self {
        Self {
            warm_up,
            started: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl SpeechProvider for TimedSpeech {
    fn describe(&self) -> Description {
        Description {
            backend: "timed-test",
            label: "Timed test voice".to_owned(),
            playback: Playback::Audio,
            local_only: false,
        }
    }

    async fn warm_up(&self) -> Result<(), SpeechError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.warm_up).await;
        self.finished.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn speak(&self, text: &str, _speed: Option<f64>) -> Result<Speech, SpeechError> {
        Ok(Speech {
            audio: text.as_bytes().to_vec(),
            content_type: "audio/test".to_owned(),
        })
    }
}

fn harness(speech: Arc<TimedSpeech>) -> (axum::Router, AppState, Arc<SlowChat>) {
    let (mut state, _chat, _elevenlabs) =
        testing::state_from_toml(&testing::config_toml_without_elevenlabs());
    let chat = Arc::new(SlowChat::default());
    state.replace_chat(chat.clone());
    state.speech = speech;
    (vibe_talk::http::router(state.clone()), state, chat)
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {READ_TOKEN}"))
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("router");
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

fn prepare_path() -> String {
    format!("/api/v1/channels/{READ_CHANNEL}/speech/prepare")
}

fn prepared_ids(body: &Value) -> Vec<String> {
    body["prepared"]
        .as_array()
        .expect("prepared list")
        .iter()
        .map(|entry| entry["message_id"].as_str().expect("id").to_owned())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_window_the_page_just_fetched_is_prepared_from_memory_without_a_fetch() {
    let (app, _state, chat) = harness(Arc::new(TimedSpeech::new(Duration::ZERO)));
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/page?limit=20"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(chat.windows.load(Ordering::SeqCst), 1);

    let began = tokio::time::Instant::now();
    let (status, body) = call(
        &app,
        "POST",
        &prepare_path(),
        Some(json!({"ids": ["101", "103"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(prepared_ids(&body), ["101", "103"]);
    assert_eq!(
        chat.windows.load(Ordering::SeqCst),
        1,
        "preparation fetched the window the page had just fetched"
    );
    assert!(
        began.elapsed() < FETCH,
        "the prepare took {:?}, as long as a fetch",
        began.elapsed()
    );
}

#[tokio::test(start_paused = true)]
async fn a_thread_view_the_page_fetched_answers_only_for_that_thread() {
    let (app, _state, chat) = harness(Arc::new(TimedSpeech::new(Duration::ZERO)));
    let encoded = THREAD.replace('/', "%2F");
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=thread&thread_id={encoded}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &app,
        "POST",
        &prepare_path(),
        Some(json!({"ids": ["201", "202"], "thread_id": THREAD})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(prepared_ids(&body), ["201", "202"]);
    assert_eq!(
        chat.lookups.load(Ordering::SeqCst),
        0,
        "the thread view was fetched again"
    );

    // The same ids under another thread are not answered from memory. They go to the backend,
    // which checks thread membership and refuses.
    let (status, body) = call(
        &app,
        "POST",
        &prepare_path(),
        Some(json!({"ids": ["201"], "thread_id": "another-thread"})),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(chat.lookups.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn preparations_that_miss_together_share_one_window_fetch() {
    let (app, _state, chat) = harness(Arc::new(TimedSpeech::new(Duration::ZERO)));
    let began = tokio::time::Instant::now();
    // The batch for the whole screen, and the tapped row on its own a moment later.
    let path = prepare_path();
    let batch = call(
        &app,
        "POST",
        &path,
        Some(json!({"ids": ["101", "102", "103"]})),
    );
    let tap = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        call(&app, "POST", &path, Some(json!({"ids": ["102"]}))).await
    };
    let ((batch_status, batch_body), (tap_status, tap_body)) = tokio::join!(batch, tap);
    assert_eq!(batch_status, StatusCode::OK, "{batch_body}");
    assert_eq!(tap_status, StatusCode::OK, "{tap_body}");
    assert_eq!(prepared_ids(&batch_body), ["101", "102", "103"]);
    assert_eq!(prepared_ids(&tap_body), ["102"]);
    assert_eq!(
        chat.windows.load(Ordering::SeqCst),
        1,
        "the second preparation queued a second identical window fetch"
    );
    assert!(
        began.elapsed() < FETCH * 2,
        "both were answered after {:?}: the fetches ran one after the other",
        began.elapsed()
    );

    // An id that is not in the window is simply absent, as it always was.
    let (status, body) = call(&app, "POST", &path, Some(json!({"ids": ["101", "999"]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(prepared_ids(&body), ["101"]);
}

#[tokio::test(start_paused = true)]
async fn missing_thread_messages_are_fetched_a_few_at_a_time_and_then_remembered() {
    let (app, _state, chat) = harness(Arc::new(TimedSpeech::new(Duration::ZERO)));
    let ids: Vec<String> = (201..=210).map(|n| n.to_string()).collect();
    let began = tokio::time::Instant::now();
    let (status, body) = call(
        &app,
        "POST",
        &prepare_path(),
        Some(json!({"ids": ids, "thread_id": THREAD})),
    )
    .await;
    let took = began.elapsed();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(prepared_ids(&body), ids, "the order on screen was not kept");
    assert_eq!(chat.lookups.load(Ordering::SeqCst), ids.len());
    assert_eq!(
        chat.most_lookups_in_flight.load(Ordering::SeqCst),
        THREAD_FETCH_CONCURRENCY,
        "the lookups were not concurrent, or not bounded"
    );
    let rounds = u32::try_from(ids.len().div_ceil(THREAD_FETCH_CONCURRENCY)).expect("small");
    assert_eq!(took, LOOKUP * rounds, "ten lookups took {took:?}");

    // A second preparation of the same messages, moments later, fetches none of them.
    let (status, body) = call(
        &app,
        "POST",
        &prepare_path(),
        Some(json!({"ids": ["203", "209"], "thread_id": THREAD})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(prepared_ids(&body), ["203", "209"]);
    assert_eq!(chat.lookups.load(Ordering::SeqCst), ids.len());
}

#[tokio::test(start_paused = true)]
async fn preparation_starts_the_warm_up_and_does_not_wait_for_it() {
    let warm_up = Duration::from_secs(10);
    let speech = Arc::new(TimedSpeech::new(warm_up));
    let (app, state, _chat) = harness(speech.clone());
    // Fetched first, so the prepare itself has nothing to wait for but the warm-up.
    call(
        &app,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/page"),
        None,
    )
    .await;

    let began = tokio::time::Instant::now();
    let (status, body) = call(&app, "POST", &prepare_path(), Some(json!({"ids": ["101"]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        began.elapsed() < warm_up,
        "the prepare answered after {:?}, so it waited for the warm-up",
        began.elapsed()
    );
    tokio::task::yield_now().await;
    assert_eq!(
        speech.started.load(Ordering::SeqCst),
        1,
        "no warm-up was started"
    );
    assert_eq!(speech.finished.load(Ordering::SeqCst), 0);
    assert!(state.read_ahead.warming());

    // A second prepare while it runs does not start another.
    let (status, _) = call(&app, "POST", &prepare_path(), Some(json!({"ids": ["102"]}))).await;
    assert_eq!(status, StatusCode::OK);
    tokio::task::yield_now().await;
    assert_eq!(
        speech.started.load(Ordering::SeqCst),
        1,
        "two warm-ups ran at once"
    );

    // Once it has finished, the next prepare warms up again: the session may have aged out.
    tokio::time::sleep(warm_up + Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(speech.finished.load(Ordering::SeqCst), 1);
    assert!(!state.read_ahead.warming());
    let (status, _) = call(&app, "POST", &prepare_path(), Some(json!({"ids": ["103"]}))).await;
    assert_eq!(status, StatusCode::OK);
    tokio::task::yield_now().await;
    assert_eq!(speech.started.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn a_prepared_ticket_from_memory_still_plays_the_message_text() {
    let (app, _state, _chat) = harness(Arc::new(TimedSpeech::new(Duration::ZERO)));
    call(
        &app,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/page"),
        None,
    )
    .await;
    let (status, body) = call(&app, "POST", &prepare_path(), Some(json!({"ids": ["102"]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let url = body["prepared"][0]["url"].as_str().expect("ticket url");
    let request = Request::builder()
        .uri(url)
        .body(Body::empty())
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("router");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(String::from_utf8_lossy(&bytes), "words of message 102");
}
