//! A vendor client the test harness built stays on loopback even where the environment names a
//! proxy. `#359 provider-diagnostics-ids`.
//!
//! A client that honoured `HTTPS_PROXY` would carry its request past the loopback-only resolver,
//! to wherever that proxy reaches. This is its own test binary because it sets the variable, which
//! every client built in the process reads; and it fails without the harness's confinement on any
//! host, network or none, because the proxy here is a loopback listener that counts connections.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::providers::ChatRouter;
use vibe_talk::testing::WRITE_TOKEN;

#[tokio::test]
async fn a_client_the_harness_built_never_goes_to_a_proxy_named_in_the_environment() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let proxy = listener.local_addr().expect("address");
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&connections);
    let accepting = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(socket);
        }
    });
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        std::env::remove_var(name);
    }
    for name in ["HTTPS_PROXY", "https_proxy"] {
        std::env::set_var(name, format!("http://{proxy}"));
    }

    // The legacy `[discord]` section names no `api_base`: a real client for the public API.
    let (mut state, _discord, _vendor) =
        vibe_talk::testing::state_from_toml(&vibe_talk::testing::config_toml());
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    let request = Request::builder()
        .method("GET")
        .uri("/api/v1/diagnostics")
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .body(Body::empty())
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    accepting.abort();

    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "a request went to the proxy: {body:#}"
    );
    let token = body["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["id"] == "discord.token")
        .expect("a Discord token check");
    assert!(
        token["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("never reached")),
        "{token:#}"
    );
}
