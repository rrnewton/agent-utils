//! `GET /api/v1/channels/{id}/recap`: a window grouped by thread with per-message Done tags.
//! `#717 vt-unread-recap`.
//!
//! Done is per message, never per thread: the fixtures dismiss a thread reply without its root
//! and a root without its reply, and assert the tags stay independent.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::store::StateStore as _;
use vibe_talk::testing::{self, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::threads::{MessageThread, TimelinePage, TimelineRequest, TimelineView};

const HOUR: i64 = 3_600_000;
const THREAD_A: &str = "thread-A";
const THREAD_B: &str = "thread-B";

fn base_ms() -> i64 {
    vibe_talk::clock::instant_ms("2026-09-20T00:00:00Z").expect("base instant")
}

fn at(hours: i64, seq: u64) -> (i64, MessageId) {
    let ms = base_ms() + hours * HOUR;
    let numeric = MessageId::at_time_ms(ms).numeric().expect("snowflake") + seq;
    (ms, MessageId(numeric.to_string()))
}

fn message(
    hours: i64,
    seq: u64,
    author: &str,
    content: &str,
    thread: Option<(&str, bool)>,
    noise: bool,
) -> Message {
    let (ms, id) = at(hours, seq);
    Message {
        edited_timestamp: None,
        id,
        channel_id: ChannelId(WRITE_CHANNEL.to_owned()),
        author: author.to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: vibe_talk::clock::iso_from_ms(ms),
        spoken_time: String::new(),
        reply_to: None,
        content: content.to_owned(),
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

/// Thread A predates the window (root at +6h, replies +7..+9h, then +11h/+12h inside it).
/// Thread B starts inside it (+13h root, +14h reply). The window is +10h..+20h.
fn fixtures() -> Vec<Message> {
    vec![
        message(6, 1, "agent-a", "A root", Some((THREAD_A, true)), false),
        message(7, 2, "agent-a", "A reply 1", Some((THREAD_A, false)), false),
        message(8, 3, "agent-a", "A reply 2", Some((THREAD_A, false)), false),
        message(9, 4, "agent-a", "A reply 3", Some((THREAD_A, false)), false),
        message(
            11,
            5,
            "agent-a",
            "A reply 4",
            Some((THREAD_A, false)),
            false,
        ),
        message(
            12,
            6,
            "agent-b",
            "A reply 5",
            Some((THREAD_A, false)),
            false,
        ),
        message(13, 7, "agent-a", "B root", Some((THREAD_B, true)), false),
        message(14, 8, "agent-b", "B reply", Some((THREAD_B, false)), false),
        message(10, 9, "agent-a", "standalone one", None, false),
        message(15, 10, "agent-b", "standalone two", None, false),
        message(11, 11, "agent-a", "Working…", Some((THREAD_A, false)), true),
    ]
}

#[derive(Default)]
struct RecapBackend {
    messages: Mutex<Vec<Message>>,
}

#[async_trait::async_trait]
impl ChatClient for RecapBackend {
    fn provider_name(&self) -> &str {
        "Recap Test"
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

async fn harness() -> (axum::Router, Vec<Message>) {
    let (mut state, _discord, store) = testing::state_with_store();
    let backend = Arc::new(RecapBackend::default());
    let messages = fixtures();
    *backend.messages.lock().expect("messages") = messages.clone();
    state.replace_chat(backend);
    // A reply Done without its root; a root Done without its reply; one standalone Done.
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    store
        .dismiss(
            &channel,
            &[
                messages[4].id.clone(),
                messages[6].id.clone(),
                messages[9].id.clone(),
            ],
        )
        .await
        .expect("dismiss");
    store
        .mark_read_at(&channel, &messages[8].id, None)
        .await
        .expect("read mark");
    (vibe_talk::http::router(state), messages)
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request.body(Body::empty()).expect("request");
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

fn window_path() -> String {
    format!(
        "/api/v1/channels/{WRITE_CHANNEL}/recap?since_ms={}&until_ms={}",
        base_ms() + 10 * HOUR,
        base_ms() + 20 * HOUR
    )
}

#[tokio::test]
async fn recap_tags_done_per_message_and_groups_by_thread() {
    let (app, messages) = harness().await;
    let (status, body) = call(&app, "GET", &window_path(), Some(READ_TOKEN)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["counts"]["done"], json!(3), "{body}");
    assert_eq!(body["counts"]["unread"], json!(3), "{body}");
    assert_eq!(body["counts"]["noise"], json!(1), "{body}");
    assert_eq!(
        body["read_mark"]["last_read"],
        json!(messages[8].id.as_str()),
        "{body}"
    );
    let groups = body["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 4, "{body}");
    // Standalone one (+10h) first; thread A's first in-window reply is +11h.
    assert_eq!(groups[0]["thread_id"], Value::Null);
    assert_eq!(groups[0]["messages"][0]["done"], json!(false));
    let thread_a = &groups[1];
    assert_eq!(thread_a["thread_id"], json!(THREAD_A));
    assert_eq!(thread_a["hidden_before"], json!(1), "{thread_a}");
    let context = thread_a["context"].as_array().expect("context");
    assert_eq!(
        context.len(),
        3,
        "root plus two opening replies: {thread_a}"
    );
    assert_eq!(context[0]["content"], json!("A root"));
    assert_eq!(
        context[0]["done"],
        json!(false),
        "the root is not Done just because a reply is"
    );
    let in_window = thread_a["messages"].as_array().expect("messages");
    assert_eq!(in_window.len(), 2, "{thread_a}");
    assert_eq!(in_window[0]["content"], json!("A reply 4"));
    assert_eq!(in_window[0]["done"], json!(true), "the dismissed reply");
    assert_eq!(in_window[1]["done"], json!(false));
    let thread_b = &groups[2];
    assert_eq!(thread_b["thread_id"], json!(THREAD_B));
    assert_eq!(thread_b["hidden_before"], json!(0));
    assert_eq!(
        thread_b["messages"][0]["done"],
        json!(true),
        "the dismissed root"
    );
    assert_eq!(
        thread_b["messages"][1]["done"],
        json!(false),
        "its reply is not Done with it"
    );
    assert_eq!(groups[3]["messages"][0]["content"], json!("standalone two"));
    assert_eq!(groups[3]["messages"][0]["done"], json!(true));
}

#[tokio::test]
async fn recap_refuses_bad_windows_and_unknown_channels() {
    let (app, _messages) = harness().await;
    assert_eq!(
        call(&app, "GET", &window_path(), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let unknown = window_path().replace(WRITE_CHANNEL, "9999999999");
    assert_eq!(
        call(&app, "GET", &unknown, Some(READ_TOKEN)).await.0,
        StatusCode::NOT_FOUND
    );
    let both = format!(
        "/api/v1/channels/{WRITE_CHANNEL}/recap?since_ms={}&hours=2",
        base_ms()
    );
    assert_eq!(
        call(&app, "GET", &both, Some(WRITE_TOKEN)).await.0,
        StatusCode::BAD_REQUEST
    );
    let empty = format!(
        "/api/v1/channels/{WRITE_CHANNEL}/recap?since_ms={}&until_ms={}",
        base_ms() + HOUR,
        base_ms()
    );
    assert_eq!(
        call(&app, "GET", &empty, Some(READ_TOKEN)).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn recap_without_a_store_annotates_nothing() {
    let (mut state, _discord) =
        testing::state_with(Arc::new(vibe_talk::store::disabled::DisabledStore));
    let backend = Arc::new(RecapBackend::default());
    *backend.messages.lock().expect("messages") = fixtures();
    state.replace_chat(backend);
    let app = vibe_talk::http::router(state);
    let (status, body) = call(&app, "GET", &window_path(), Some(READ_TOKEN)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["counts"]["done"], json!(0), "{body}");
    assert_eq!(body["counts"]["unread"], json!(6), "{body}");
    assert!(body.get("read_mark").is_none(), "{body}");
}
