//! `#1840 vibe-talk-draft-sync-across-devices`, end to end through the real router.
//!
//! A draft typed on one device is offered on another, sending clears it, a
//! read token never reaches it, and the storage purge drops it.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::http::router;
use vibe_talk::testing::{READ_CHANNEL, READ_TOKEN, WRITE_TOKEN};

fn harness() -> axum::Router {
    let (state, _discord, _store) = vibe_talk::testing::state_with_store();
    router(state)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json");
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
    let response = app.clone().oneshot(request).await.expect("router responds");
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

fn drafts_uri() -> String {
    format!("/api/v1/channels/{READ_CHANNEL}/drafts")
}

#[tokio::test]
async fn a_draft_from_one_device_is_offered_on_another_and_sending_clears_it() {
    let app = harness();
    // Device A idles with a half-written message; the page syncs it.
    let (status, answer) = call(
        &app,
        "PUT",
        &drafts_uri(),
        Some(WRITE_TOKEN),
        Some(json!({
            "scope": "main",
            "text": "a long half-written message",
            "device_id": "device-a",
            "device_label": "phone"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["draft"]["text"], "a long half-written message");
    assert_eq!(answer["draft"]["device_label"], "phone");
    assert_eq!(answer["replaced"], Value::Null);
    // Device B opens the channel and is offered the words, with their origin.
    let (status, answer) = call(&app, "GET", &drafts_uri(), Some(WRITE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    let drafts = answer["drafts"].as_array().expect("drafts");
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0]["text"], "a long half-written message");
    assert_eq!(drafts[0]["device_id"], "device-a");
    // Sending clears it, as the page's DELETE after a confirmed post does.
    let (status, _) = call(
        &app,
        "DELETE",
        &drafts_uri(),
        Some(WRITE_TOKEN),
        Some(json!({ "scope": "main" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, answer) = call(&app, "GET", &drafts_uri(), Some(WRITE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(answer["drafts"].as_array().expect("drafts").is_empty());
}

#[tokio::test]
async fn a_read_token_never_reaches_drafts() {
    let app = harness();
    for method in ["GET", "PUT", "DELETE"] {
        let (status, _) = call(
            &app,
            method,
            &drafts_uri(),
            Some(READ_TOKEN),
            Some(json!({
                "scope": "main",
                "text": "hi",
                "device_id": "device-a",
                "device_label": "phone"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} with a read token");
    }
    let (status, _) = call(&app, "GET", &drafts_uri(), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_purge_drops_drafts() {
    let app = harness();
    let (status, _) = call(
        &app,
        "PUT",
        &drafts_uri(),
        Some(WRITE_TOKEN),
        Some(json!({
            "scope": "discuss",
            "thread_id": "1700.0001",
            "text": "a follow-up in progress",
            "device_id": "device-a",
            "device_label": "phone"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, answer) = call(&app, "DELETE", "/api/v1/storage", Some(WRITE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(
        answer["purged"]
            .as_array()
            .expect("purged")
            .iter()
            .any(|name| name == "composer_drafts"),
        "{answer}"
    );
    let (status, answer) = call(&app, "GET", &drafts_uri(), Some(WRITE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(answer["drafts"].as_array().expect("drafts").is_empty());
}
