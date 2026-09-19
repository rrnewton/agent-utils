//! Provider names must survive the real HTTP client and both API error paths.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, RateLimitExhausted};
use vibe_talk::discord::http::HttpDiscordClient;
use vibe_talk::http::router;
use vibe_talk::model::{ChannelId, MessageId};
use vibe_talk::testing::{self, WRITE_CHANNEL, WRITE_TOKEN};

async fn upstream(status: StatusCode, body: String) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let app = axum::Router::new().fallback(move || {
        let body = body.clone();
        async move { (status, body) }
    });
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    (base, task)
}

#[tokio::test]
async fn google_chat_metadata_and_failed_reads_and_sends_use_the_backend_name() {
    // A multibyte response also exercises truncation: slicing at byte 500 used to panic.
    let (base, task) = upstream(StatusCode::BAD_GATEWAY, "故障".repeat(300)).await;
    let mut config = testing::config();
    config.discord.api_base = base;
    config.discord.provider_name = "Google Chat".to_owned();
    let (mut state, _) = testing::state();
    state.chat = Arc::new(HttpDiscordClient::new(&config.discord).expect("client"));
    // Leave state.config at its default Discord value to prove that the trait supplies the name.
    let app = router(state);
    for (method, uri, body) in [
        ("GET", "/api/v1/client-config".to_owned(), None),
        (
            "GET",
            format!("/api/v1/channels/{WRITE_CHANNEL}/page"),
            None,
        ),
        (
            "POST",
            format!("/api/v1/channels/{WRITE_CHANNEL}/reply"),
            Some(r#"{"text":"hello"}"#),
        ),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(&uri)
            .header("authorization", format!("Bearer {WRITE_TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::from(body.unwrap_or_default()))
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        if uri == "/api/v1/client-config" {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(value["chat_provider_name"], "Google Chat");
        } else {
            assert_eq!(
                status,
                if method == "POST" {
                    StatusCode::MULTI_STATUS
                } else {
                    StatusCode::BAD_GATEWAY
                },
                "{value}"
            );
            assert!(
                value["detail"]
                    .as_str()
                    .expect("detail")
                    .starts_with("Google Chat returned HTTP 502:"),
                "{value}"
            );
            assert!(
                !value.to_string().to_lowercase().contains("discord"),
                "{value}"
            );
            if method == "GET" {
                assert_eq!(value["error"], "chat_error");
            } else {
                assert_eq!(value["posted"], 0);
                assert_eq!(value["unsent"], "hello");
            }
        }
    }
    task.abort();
}

#[tokio::test]
async fn malformed_successes_and_refusals_retain_provider_and_classification() {
    let (base, task) = upstream(StatusCode::OK, "{}".to_owned()).await;
    let mut config = testing::config();
    config.discord.api_base = base;
    config.discord.provider_name = "Google Chat".to_owned();
    let client = HttpDiscordClient::new(&config.discord).expect("client");
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    for error in [
        client.identity().await.expect_err("missing identity"),
        client
            .fetch_recent(&channel, 1)
            .await
            .expect_err("missing list"),
        client
            .post_message(&channel, "hello", None)
            .await
            .expect_err("missing message"),
    ] {
        assert!(matches!(error.cause(), ChatError::Shape(_)), "{error}");
        assert!(error
            .to_string()
            .starts_with("Google Chat response could not be understood:"));
    }
    let refused = client
        .mark_read_upstream(&channel, &MessageId("123".to_owned()))
        .await
        .expect_err("disabled");
    assert!(matches!(refused.cause(), ChatError::Refused(_)));
    assert!(refused.to_string().starts_with("Google Chat refused:"));
    task.abort();
}

#[test]
fn provider_context_preserves_rate_limit_backoff() {
    let wait = Duration::from_secs(45);
    let error = ChatError::RateLimited(RateLimitExhausted {
        provider: "discord",
        route: "GET /channels/123/messages".to_owned(),
        attempts: 1,
        waited: Duration::ZERO,
        budget: Duration::from_secs(30),
        retry_after: wait,
        global: false,
    })
    .with_provider("Google Chat");
    assert_eq!(error.retry_after(), Some(wait));
    assert!(error.to_string().starts_with("Google Chat RATE LIMIT"));
    assert!(!error.to_string().contains("discord"));
    assert!(ChatError::Transport("offline".to_owned())
        .with_provider("Google Chat")
        .to_string()
        .starts_with("Google Chat request failed:"));
}

#[test]
fn provider_configuration_defaults_and_validates_the_display_name() {
    let parse = |name: &str| {
        let toml = testing::config_toml()
            .replace("[discord]", &format!("[discord]\nprovider_name = {name}"));
        vibe_talk::config::Config::from_toml_and_env(&toml, &Default::default())
    };
    assert_eq!(testing::config().discord.provider_name, "Discord");
    assert_eq!(
        parse("\" Google Chat \"")
            .expect("valid")
            .discord
            .provider_name,
        "Google Chat"
    );
    assert!(parse("\" \"").is_err());
    assert!(parse("\"Google\\nChat\"").is_err());
}
