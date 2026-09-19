//! Thread views and writes keep the configured parent channel's authorization boundary.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::testing::{self, READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::threads::{
    MessageThread, ThreadSummary, TimelinePage, TimelineRequest, TimelineView,
};

const THREAD: &str = "opaque/thread-A";

fn message(id: &str, content: &str, thread: bool) -> Message {
    Message {
        id: MessageId(id.to_owned()),
        channel_id: ChannelId(WRITE_CHANNEL.to_owned()),
        author: "Reader".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: "2026-09-20T01:00:00Z".to_owned(),
        spoken_time: String::new(),
        reply_to: None,
        content: content.to_owned(),
        thread: thread.then(|| MessageThread {
            id: THREAD.to_owned(),
            root_message_id: Some(MessageId("10".to_owned())),
            is_root: id == "10",
            reply_count: Some(2),
            reply_count_exact: true,
        }),
    }
}

type RecordedPost = (String, Option<String>, String, Option<String>);

#[derive(Default)]
struct ThreadBackend {
    reads: Mutex<Vec<(String, TimelineRequest)>>,
    posts: Mutex<Vec<RecordedPost>>,
    lookups: Mutex<Vec<(String, String, String)>>,
}

impl ThreadBackend {
    fn check_thread(thread: &str) -> Result<(), ChatError> {
        if thread != THREAD {
            return Err(
                ChatError::Refused("thread is outside this channel".to_owned())
                    .with_provider("Google Chat"),
            );
        }
        Ok(())
    }

    fn post(
        &self,
        channel: &ChannelId,
        thread: Option<&str>,
        content: &str,
        reply: Option<&MessageId>,
    ) -> Message {
        self.posts.lock().unwrap().push((
            channel.0.clone(),
            thread.map(str::to_owned),
            content.to_owned(),
            reply.map(|id| id.0.clone()),
        ));
        let mut posted = message("30", content, thread.is_some());
        posted.reply_to = reply.cloned();
        posted
    }
}

#[async_trait::async_trait]
impl ChatClient for ThreadBackend {
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
        Ok(vec![message("20", "main message", false)])
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        self.reads
            .lock()
            .unwrap()
            .push((channel.0.clone(), request.clone()));
        if let Some(thread) = &request.thread_id {
            Self::check_thread(thread)?;
        }
        let root = message("10", "root", true);
        let summary = ThreadSummary {
            id: THREAD.to_owned(),
            root: Some(root.clone()),
            title: "root".to_owned(),
            reply_count: Some(2),
            reply_count_exact: true,
            updated_at: "2026-09-20T01:00:00Z".to_owned(),
        };
        Ok(TimelinePage {
            messages: if request.view == TimelineView::Threads {
                vec![]
            } else {
                vec![root, message("11", "older thread text", true)]
            },
            threads: if request.view == TimelineView::Threads {
                vec![summary.clone()]
            } else {
                vec![]
            },
            thread: (request.view == TimelineView::Thread).then_some(summary),
            has_threads: true,
            has_more: true,
            next_before: Some("opaque cursor/+=".to_owned()),
            notice: None,
        })
    }

    async fn fetch_thread_message(
        &self,
        channel: &ChannelId,
        thread: &str,
        id: &MessageId,
    ) -> Result<Message, ChatError> {
        Self::check_thread(thread)?;
        self.lookups
            .lock()
            .unwrap()
            .push((channel.0.clone(), thread.to_owned(), id.0.clone()));
        Ok(message(&id.0, "older thread text", true))
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Ok(self.post(channel, None, content, reply))
    }

    async fn post_in_thread(
        &self,
        channel: &ChannelId,
        thread: &str,
        content: &str,
        reply: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Self::check_thread(thread)?;
        Ok(self.post(channel, Some(thread), content, reply))
    }
}

fn harness() -> (axum::Router, Arc<ThreadBackend>) {
    let (mut state, _) = testing::state();
    let backend = Arc::new(ThreadBackend::default());
    state.chat = backend.clone();
    (vibe_talk::http::router(state), backend)
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request
        .header("content-type", "application/json")
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
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
async fn timeline_authorizes_parent_before_asking_for_any_thread() {
    let (app, backend) = harness();
    for view in [
        "main",
        "threads",
        "flat",
        "thread&thread_id=opaque%2Fthread-A",
    ] {
        let path = format!("/api/v1/channels/{WRITE_CHANNEL}/timeline?view={view}");
        assert_eq!(
            call(&app, "GET", &path, None, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        let unknown = path.replace(WRITE_CHANNEL, "unknown");
        assert_eq!(
            call(&app, "GET", &unknown, Some(READ_TOKEN), None).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert!(backend.reads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn timeline_passes_opaque_context_and_stamps_message_and_root_times() {
    let (app, backend) = harness();
    let path=format!("/api/v1/channels/{WRITE_CHANNEL}/timeline?view=thread&thread_id=opaque%2Fthread-A&before=opaque%20cursor%2F%2B%3D&limit=999");
    let (status, body) = call(&app, "GET", &path, Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["next_before"], "opaque cursor/+=");
    assert_eq!(body["thread"]["id"], THREAD);
    assert_eq!(body["thread"]["reply_count"], 2);
    assert!(!body["messages"][0]["spoken_time"]
        .as_str()
        .unwrap()
        .is_empty());
    assert!(!body["thread"]["root"]["spoken_time"]
        .as_str()
        .unwrap()
        .is_empty());
    let reads = backend.reads.lock().unwrap();
    assert_eq!(reads[0].1.thread_id.as_deref(), Some(THREAD));
    assert_eq!(reads[0].1.before.as_deref(), Some("opaque cursor/+="));
    assert_eq!(reads[0].1.limit, 50);
}

#[tokio::test]
async fn invalid_context_is_rejected_instead_of_falling_back_to_main() {
    let (app, backend) = harness();
    for suffix in [
        "view=thread",
        "view=main&thread_id=a",
        "view=thread&thread_id=",
        "view=flat&before=",
    ] {
        let path = format!("/api/v1/channels/{WRITE_CHANNEL}/timeline?{suffix}");
        assert_eq!(
            call(&app, "GET", &path, Some(READ_TOKEN), None).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(backend.reads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn standalone_and_thread_composers_route_without_inventing_reply_targets() {
    let (app, backend) = harness();
    let path = format!("/api/v1/channels/{WRITE_CHANNEL}/reply");
    for body in [
        json!({"text":"new main message"}),
        json!({"text":"new thread message","thread_id":THREAD}),
    ] {
        assert_eq!(
            call(&app, "POST", &path, Some(WRITE_TOKEN), Some(body))
                .await
                .0,
            StatusCode::OK
        );
    }
    let posts = backend.posts.lock().unwrap();
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[0].1, None);
    assert_eq!(posts[1].1.as_deref(), Some(THREAD));
    assert!(posts.iter().all(|p| p.3.is_none()));
}

#[tokio::test]
async fn thread_writes_keep_write_scope_parent_policy_and_membership() {
    let (app, backend) = harness();
    let body = json!({"text":"must not post","thread_id":THREAD});
    for (channel, token, status) in [
        (WRITE_CHANNEL, Some(READ_TOKEN), StatusCode::FORBIDDEN),
        (WRITE_CHANNEL, None, StatusCode::UNAUTHORIZED),
        (READ_CHANNEL, Some(WRITE_TOKEN), StatusCode::FORBIDDEN),
        ("unknown", Some(WRITE_TOKEN), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            call(
                &app,
                "POST",
                &format!("/api/v1/channels/{channel}/reply"),
                token,
                Some(body.clone())
            )
            .await
            .0,
            status
        );
    }
    let (_, failure) = call(
        &app,
        "POST",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/reply"),
        Some(WRITE_TOKEN),
        Some(json!({"text":"must not post","thread_id":"foreign-thread"})),
    )
    .await;
    assert!(failure["detail"].as_str().unwrap().contains("Google Chat"));
    assert_eq!(failure["posted"], 0);
    assert!(backend.posts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn every_part_of_a_long_thread_message_stays_in_its_thread() {
    let (app, backend) = harness();
    let text = "message ".repeat(500);
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/reply"),
        Some(WRITE_TOKEN),
        Some(json!({"text":text,"thread_id":THREAD,"reply_to":"10"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let posts = backend.posts.lock().unwrap();
    assert!(posts.len() > 1);
    assert!(posts.iter().all(|p| p.1.as_deref() == Some(THREAD)));
    assert_eq!(posts[0].3.as_deref(), Some("10"));
    assert!(posts[1..].iter().all(|p| p.3.is_none()));
    assert_eq!(posts.iter().map(|p| p.2.as_str()).collect::<String>(), text);
}

#[tokio::test]
async fn older_thread_messages_remain_addressable_for_summaries_and_speech() {
    let (app, backend) = harness();
    let prefix = format!("/api/v1/channels/{WRITE_CHANNEL}");
    for suffix in [
        "/messages/11?thread_id=opaque%2Fthread-A",
        "/messages/11/summary?thread_id=opaque%2Fthread-A",
    ] {
        let (status, body) = call(
            &app,
            "GET",
            &(prefix.clone() + suffix),
            Some(READ_TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, body) = call(
        &app,
        "POST",
        &(prefix + "/speech/prepare"),
        Some(READ_TOKEN),
        Some(json!({"ids":["11"],"thread_id":THREAD})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["prepared"][0]["message_id"], "11");
    assert_eq!(backend.lookups.lock().unwrap().len(), 3);
    let (_, config) = call(&app, "GET", "/api/v1/client-config", Some(READ_TOKEN), None).await;
    assert_eq!(config["threading_supported"], true);
    assert_eq!(config["chat_provider_name"], "Google Chat");
}
