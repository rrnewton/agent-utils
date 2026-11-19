//! `#1825 vibe-talk-hashtag-labels`: hashtags become labels over HTTP, and
//! label groups (owner thread 167) rename them live.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::http::router;
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::testing::{self, READ_CHANNEL, READ_TOKEN, WRITE_TOKEN};
use vibe_talk::threads::{TimelinePage, TimelineRequest};

/// One message carrying two hashtags, one of them a grouping candidate.
struct Tagged;

fn message(id: &str) -> Message {
    Message {
        edited_timestamp: None,
        thread: None,
        id: MessageId(id.to_owned()),
        channel_id: ChannelId(READ_CHANNEL.to_owned()),
        author: "build-bot".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: true,
        timestamp: "2026-10-04T12:00:00Z".to_owned(),
        spoken_time: String::new(),
        reply_to: None,
        content: "fixed the stall\n#benchmark #kvm".to_owned(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        labels: Vec::new(),
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

#[async_trait::async_trait]
impl ChatClient for Tagged {
    fn provider_name(&self) -> &str {
        "Example chat"
    }

    fn supports_threading(&self) -> bool {
        true
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        Ok(ChatIdentity {
            id: "7".to_owned(),
            username: "reader".to_owned(),
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
        Ok(vec![message("1000000000000000100")])
    }

    async fn fetch_timeline(
        &self,
        _channel: &ChannelId,
        _request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        Ok(TimelinePage {
            messages: vec![message("1000000000000000100")],
            ..TimelinePage::default()
        })
    }

    async fn post_message(
        &self,
        _channel: &ChannelId,
        _content: &str,
        _reply: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused("this backend does not post".to_owned()))
    }
}

struct Harness {
    router: axum::Router,
}

impl Harness {
    fn new() -> Self {
        let (mut state, _discord, _store) = testing::state_with_store();
        state.replace_chat(Arc::new(Tagged));
        Self {
            router: router(state),
        }
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let request = match body {
            Some(json) => builder
                .header("content-type", "application/json")
                .body(Body::from(json.to_string()))
                .expect("request"),
            None => builder.body(Body::empty()).expect("request"),
        };
        let response = self
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

    async fn served_labels(&self) -> Vec<String> {
        let (status, body) = self
            .call(
                "GET",
                &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main"),
                Some(READ_TOKEN),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["messages"][0]["labels"]
            .as_array()
            .expect("labels served")
            .iter()
            .map(|label| label.as_str().expect("a slug").to_owned())
            .collect()
    }
}

#[tokio::test]
async fn pinned_is_the_builtin_label_over_http_and_pinned_the_slug_is_reserved() {
    // `#1853`: the page's exact pin write and pins read over the real router,
    // the reserved slug refused as a group, and no hashtag named pinned.
    let harness = Harness::new();
    let id = "1000000000000000100";
    let (status, change) = harness
        .call(
            "PUT",
            &format!("/api/v1/channels/{READ_CHANNEL}/pins/{id}"),
            Some(WRITE_TOKEN),
            Some(json!({
                "author": "build-bot", "author_id": "7", "author_is_bot": true,
                "content": "fixed the stall\n#benchmark #kvm",
                "timestamp": "2026-10-04T12:00:00Z", "thread_id": null, "thread_root": false
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{change}");
    assert_eq!(change["pinned"], json!(true));

    let (status, pins) = harness
        .call(
            "GET",
            &format!("/api/v1/channels/{READ_CHANNEL}/pins"),
            Some(READ_TOKEN),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{pins}");
    assert_eq!(pins["pins"][0]["message_id"], json!(id), "{pins}");

    // A group may not claim the built-in's slug.
    let (status, refused) = harness
        .call(
            "POST",
            "/api/v1/label-groups",
            Some(WRITE_TOKEN),
            Some(json!({"display": "pinned", "spellings": ["pinned", "kvm"]})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(format!("{refused}").contains("reserved"), "{refused}");

    // The served labels carry the hashtags alone: no `pinned` among them,
    // before the pin and after it alike (labels are derived, pins are not).
    assert_eq!(harness.served_labels().await, vec!["benchmark", "kvm"]);
}

#[tokio::test]
async fn hashtags_are_served_as_labels_and_grouping_renames_them_live() {
    let harness = Harness::new();
    assert_eq!(harness.served_labels().await, vec!["benchmark", "kvm"]);

    // Grouping needs the write token.
    let (status, _) = harness
        .call(
            "POST",
            "/api/v1/label-groups",
            Some(READ_TOKEN),
            Some(json!({"display": "benchmarking", "spellings": ["benchmark", "benchmarking"]})),
        )
        .await;
    assert!(
        status.is_client_error(),
        "read token must not group: {status}"
    );

    let (status, made) = harness
        .call(
            "POST",
            "/api/v1/label-groups",
            Some(WRITE_TOKEN),
            Some(json!({"display": "benchmarking", "spellings": ["benchmark", "benchmarking"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["groups"][0]["display"], json!("benchmarking"));

    // Live, with no restart: the same message now serves the display.
    assert_eq!(harness.served_labels().await, vec!["benchmarking", "kvm"]);

    // A spelling already grouped refuses a second group, naming the first.
    let (status, refused) = harness
        .call(
            "POST",
            "/api/v1/label-groups",
            Some(WRITE_TOKEN),
            Some(json!({"display": "perf", "spellings": ["benchmark", "perf"]})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(
        format!("{refused}").contains("already belongs"),
        "{refused}"
    );

    // The browse lists the group; dissolve restores the typed spellings.
    let (status, browse) = harness
        .call("GET", "/api/v1/labels", Some(READ_TOKEN), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{browse}");
    assert_eq!(browse["groups"][0]["display"], json!("benchmarking"));
    let id = browse["groups"][0]["id"].as_i64().expect("group id");
    let (status, _) = harness
        .call(
            "DELETE",
            &format!("/api/v1/label-groups/{id}"),
            Some(WRITE_TOKEN),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(harness.served_labels().await, vec!["benchmark", "kvm"]);
}
