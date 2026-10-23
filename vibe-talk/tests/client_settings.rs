//! `#457 settings-server-side`, end to end through the real router.
//!
//! The settings outside Screen and keys live on the server so every device reads the
//! same choices. These drive `GET`/`PUT /api/v1/client-settings` as a page does —
//! seed once without clobbering, set with the server's stamp, forget back to the
//! default, refuse what the server does not keep — and watch the live hub carry a
//! change to the pages already open.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::http::router;
use vibe_talk::testing::{READ_TOKEN, WRITE_TOKEN};

struct Harness {
    router: axum::Router,
    state: vibe_talk::state::AppState,
}

fn harness() -> Harness {
    let (state, _discord, _store) = vibe_talk::testing::state_with_store();
    Harness {
        router: router(state.clone()),
        state,
    }
}

async fn send(
    harness: &Harness,
    method: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri("/api/v1/client-settings")
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
    let response = harness
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

fn entry<'a>(body: &'a Value, key: &str) -> Option<&'a Value> {
    body["settings"]
        .as_array()?
        .iter()
        .find(|held| held["key"] == key)
}

const KEY: &str = "vibe-talk.voice.mark-own-read";

#[tokio::test]
async fn a_seed_fills_only_what_the_server_does_not_hold() {
    let harness = harness();
    // Nothing held: an empty document, not an error — every default stands.
    let (status, body) = send(&harness, "GET", Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "settings": [] }));

    // The first device's migration lands.
    let (status, body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [], "seed": [{ "key": KEY, "value": "0", "updated_ms": 0 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let seeded = entry(&body, KEY).expect("the seeded entry").clone();
    assert_eq!(seeded["value"], "0");
    assert!(
        seeded["updated_ms"].as_i64().expect("a stamp") > 0,
        "the server did not stamp the seed: {seeded}"
    );

    // A second device's older local copy cannot move it.
    let (status, body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [], "seed": [{ "key": KEY, "value": "1", "updated_ms": 0 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(entry(&body, KEY).expect("held"), &seeded);
}

#[tokio::test]
async fn a_set_overwrites_forgets_and_is_stamped_here_not_by_the_caller() {
    let harness = harness();
    // A caller claiming a stamp far in its own future gains nothing by it.
    let (status, body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [{ "key": KEY, "value": "0", "updated_ms": 9_999_999_999_999_i64 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let held = entry(&body, KEY).expect("held").clone();
    assert_eq!(held["value"], "0");
    assert!(
        held["updated_ms"].as_i64().expect("a stamp") < 9_999_999_999_999,
        "the caller's stamp was kept: {held}"
    );

    let (status, body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [{ "key": KEY, "value": "1", "updated_ms": 0 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(entry(&body, KEY).expect("held")["value"], "1");

    // Forgotten: the key leaves the document, and the shipped default stands again.
    let (status, body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [{ "key": KEY, "value": Value::Null, "updated_ms": 0 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        entry(&body, KEY).is_none(),
        "the forgotten key stayed: {body}"
    );
}

#[tokio::test]
async fn what_the_server_does_not_keep_is_refused_and_nothing_is_stored() {
    let harness = harness();
    // A Screen and keys key: per-device, never the server's.
    let (status, body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [{ "key": "vibe-talk.voice.msg-scale", "value": "2", "updated_ms": 0 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // One bad entry refuses the write whole, the good one with it.
    let (status, _body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [
            { "key": KEY, "value": "0", "updated_ms": 0 },
            { "key": "vibe-talk.voice.width", "value": "80", "updated_ms": 0 },
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = send(&harness, "GET", Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "settings": [] }),
        "a refused write stored part"
    );
}

#[tokio::test]
async fn a_read_token_may_look_but_not_change_and_no_token_gets_nothing() {
    let harness = harness();
    let (status, _body) = send(
        &harness,
        "PUT",
        Some(READ_TOKEN),
        Some(json!({ "set": [{ "key": KEY, "value": "0", "updated_ms": 0 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _body) = send(&harness, "GET", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_change_is_published_to_the_pages_already_open() {
    let harness = harness();
    let mut listening = harness.state.live.subscribe_settings();
    let (status, _body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [{ "key": KEY, "value": "0", "updated_ms": 0 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let changed = tokio::time::timeout(std::time::Duration::from_secs(5), listening.recv())
        .await
        .expect("the change was published")
        .expect("the hub is open");
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].key, KEY);
    assert_eq!(changed[0].value.as_deref(), Some("0"));
    assert!(changed[0].updated_ms > 0);

    // A forget is published too, as no value under a stamp past the value's own —
    // the only shape a page's newer-than check can take.
    let (status, _body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [{ "key": KEY, "value": Value::Null, "updated_ms": 0 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let forgotten = tokio::time::timeout(std::time::Duration::from_secs(5), listening.recv())
        .await
        .expect("the forget was published")
        .expect("the hub is open");
    assert_eq!(forgotten.len(), 1);
    assert_eq!(forgotten[0].key, KEY);
    assert_eq!(forgotten[0].value, None);
    assert!(
        forgotten[0].updated_ms > changed[0].updated_ms,
        "the forget is not stamped past the value it forgets"
    );

    // A write that changes nothing publishes nothing: the key set again, a seed for
    // it then finds it held and loses.
    let (status, _body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [{ "key": KEY, "value": "0", "updated_ms": 0 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), listening.recv())
        .await
        .expect("the re-set was published");
    let (status, _body) = send(
        &harness,
        "PUT",
        Some(WRITE_TOKEN),
        Some(json!({ "set": [], "seed": [{ "key": KEY, "value": "1", "updated_ms": 0 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), listening.recv())
            .await
            .is_err(),
        "a seed that lost was announced as a change"
    );
}
