//! `#1848 vibe-talk-route-health-alert`: the access layer counts every
//! `/api/` answer per folded route, the health answer shows the standing,
//! and a closed window of nothing-but-refusals evaluates to one alert.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::testing::{self, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};

async fn send(app: &axum::Router, method: &str, path: &str, body: Body) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .header("content-type", "application/json")
        .body(body)
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

#[tokio::test]
async fn an_all_refused_route_counts_in_health_and_alerts_when_its_window_closes() {
    let (state, _discord, _store, _summarizer) =
        testing::state_with_summarizer(&testing::config_toml());
    let app = vibe_talk::http::router(state.clone());
    let path = format!("/api/v1/channels/{WRITE_CHANNEL}/catchup/messages");

    // Six malformed bodies: the extractor refuses each with a JSON 422.
    for _ in 0..6 {
        let (status, _body) = send(
            &app,
            "POST",
            &path,
            Body::from("\"a string, not an object\""),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    // One success on another route, and one non-API answer: neither may
    // count against the refused route.
    let (status, _body) = send(&app, "GET", "/api/v1/client-config", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _body) = send(&app, "GET", "/healthz", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);

    let (_status, health) = send(&app, "GET", "/healthz", Body::empty()).await;
    let routes = health["route_health"]["routes"]
        .as_array()
        .expect("route counts in the health answer");
    let refused = routes
        .iter()
        .find(|route| route["route"] == json!("POST /api/v1/channels/{id}/catchup/messages"))
        .expect("the refused route, folded: {routes:?}");
    assert_eq!(refused["total"], json!(6), "{routes:?}");
    assert_eq!(refused["client_errors"], json!(6), "{routes:?}");
    assert_eq!(
        health["route_health"]["alerts"],
        json!([]),
        "the window is still open: {health}"
    );

    // The tick's evaluation at the window's close raises exactly one alert.
    let alerts = state
        .route_health
        .evaluate(vibe_talk::store::now_ms() + vibe_talk::route_health::WINDOW_MS);
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert_eq!(
        alerts[0].route, "POST /api/v1/channels/{id}/catchup/messages",
        "{alerts:?}"
    );
    assert_eq!(alerts[0].dominant_status, 422, "{alerts:?}");
    let (_status, health) = send(&app, "GET", "/healthz", Body::empty()).await;
    assert_eq!(
        health["route_health"]["alerts"][0]["route"],
        json!("POST /api/v1/channels/{id}/catchup/messages"),
        "{health}"
    );

    let _ = READ_TOKEN;
}
