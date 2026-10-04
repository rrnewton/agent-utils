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
    let discord = config.discord_mut().expect("discord provider");
    discord.api_base = base;
    discord.provider_name = "Google Chat".to_owned();
    let (mut state, _) = testing::state();
    state.replace_chat(Arc::new(HttpDiscordClient::new(discord).expect("client")));
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
    let discord = config.discord_mut().expect("discord provider");
    discord.api_base = base;
    discord.provider_name = "Google Chat".to_owned();
    let client = HttpDiscordClient::new(discord).expect("client");
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
    assert_eq!(
        testing::config()
            .discord()
            .expect("discord provider")
            .provider_name,
        "Discord"
    );
    assert_eq!(
        parse("\" Google Chat \"")
            .expect("valid")
            .discord()
            .expect("discord provider")
            .provider_name,
        "Google Chat"
    );
    assert!(parse("\" \"").is_err());
    assert!(parse("\"Google\\nChat\"").is_err());
}

/// `#195 send-resilience`. The page's idempotency key reaches a registration bridge as the post's
/// nonce — which the bridge contract makes its provider request id — and reaches it IDENTICALLY on
/// a second attempt at the same text, so a retry after a lost answer names the post the first
/// attempt may already have made. The 207 says whether the failure is worth retrying at all.
#[tokio::test]
async fn a_keyed_reply_reaches_a_bridge_under_the_same_nonce_on_every_attempt() {
    use std::sync::Mutex;

    let bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let seen = Arc::clone(&bodies);
    let app = axum::Router::new().fallback(move |body: axum::body::Bytes| {
        let seen = Arc::clone(&seen);
        async move {
            let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let content = value["content"].as_str().unwrap_or_default().to_owned();
            let attempt = {
                let mut seen = seen.lock().expect("bodies");
                seen.push(value);
                seen.len()
            };
            if content == "refused words" {
                return (
                    StatusCode::BAD_REQUEST,
                    r#"{"message":"not allowed"}"#.to_owned(),
                );
            }
            if attempt == 1 {
                // The incident's shape: the upstream command timed out, and nobody knows whether
                // the message went out before it did.
                return (
                    StatusCode::BAD_GATEWAY,
                    r#"{"message":"upstream send timed out"}"#.to_owned(),
                );
            }
            let message = serde_json::json!({
                "id": format!("900{attempt}"), "channel_id": WRITE_CHANNEL, "content": content,
                "timestamp": "2026-10-04T10:20:00Z",
                "author": {"id": "7", "username": "bridge", "bot": true},
            });
            (StatusCode::OK, message.to_string())
        }
    });
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

    let mut config = testing::config();
    let discord = config.discord_mut().expect("discord provider");
    discord.api_base = base;
    discord.provider_name = "Google Chat".to_owned();
    discord.channel_registration = true;
    discord.thread_api = vibe_talk::threads::ThreadApi::Off;
    let (mut state, _) = testing::state();
    state.replace_chat(Arc::new(HttpDiscordClient::new(discord).expect("client")));
    let app = router(state);
    let call = |method: &'static str, uri: String, body: Option<Value>| {
        let app = app.clone();
        async move {
            let request = Request::builder()
                .method(method)
                .uri(&uri)
                .header("authorization", format!("Bearer {WRITE_TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
                .expect("request");
            let response = app.oneshot(request).await.expect("response");
            let status = response.status();
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).expect("json"),
            )
        }
    };
    let reply = format!("/api/v1/channels/{WRITE_CHANNEL}/reply");
    let keyed = |text: &str| serde_json::json!({"text": text, "idempotency_key": "outgoing-key_1"});

    let (status, config_body) = call("GET", "/api/v1/client-config".to_owned(), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        config_body["idempotent_posts_supported"], true,
        "{config_body}"
    );

    let (status, first) = call("POST", reply.clone(), Some(keyed("the same words"))).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{first}");
    assert_eq!(first["posted"], 0, "{first}");
    assert_eq!(
        first["retryable"], true,
        "a timed-out send was called final: {first}"
    );

    let (status, second) = call("POST", reply.clone(), Some(keyed("the same words"))).await;
    assert_eq!(status, StatusCode::OK, "{second}");

    let (status, _) = call("POST", reply.clone(), Some(keyed("other words"))).await;
    assert_eq!(status, StatusCode::OK);

    let (status, refused) = call("POST", reply.clone(), Some(keyed("refused words"))).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{refused}");
    assert_eq!(
        refused["retryable"], false,
        "a refusal was offered for retry: {refused}"
    );

    let (status, bad) = call(
        "POST",
        reply.clone(),
        Some(serde_json::json!({"text": "x", "idempotency_key": "not a key!"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");

    let nonces: Vec<String> = bodies
        .lock()
        .expect("bodies")
        .iter()
        .map(|body| {
            body["nonce"]
                .as_str()
                .expect("every bridge post has a nonce")
                .to_owned()
        })
        .collect();
    assert_eq!(nonces.len(), 4, "{nonces:?}");
    assert_eq!(
        nonces[0], nonces[1],
        "a retry of the same text minted a different request id"
    );
    assert!(nonces[0].starts_with("outgoing-key_1."), "{nonces:?}");
    assert_ne!(
        nonces[0], nonces[2],
        "one key named two different texts as the same post"
    );
    task.abort();
}

#[test]
fn a_part_key_is_bound_to_its_text_and_stays_a_valid_bridge_nonce() {
    let key = "k".repeat(vibe_talk::ops::IDEMPOTENCY_KEY_MAX_CHARS);
    let first = vibe_talk::ops::part_key(&key, "a part", 0);
    assert_eq!(first, vibe_talk::ops::part_key(&key, "a part", 0));
    assert_ne!(first, vibe_talk::ops::part_key(&key, "another part", 0));
    assert_ne!(
        first,
        vibe_talk::ops::part_key(&key, "a part", 1),
        "a repeated part would be posted once instead of twice"
    );
    assert!(first.len() <= 128, "{first}");
    assert!(first
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')));
}

#[test]
fn only_a_bridge_whose_every_post_carries_the_nonce_promises_one_post_per_key() {
    use vibe_talk::threads::ThreadApi;

    let mut config = testing::config();
    let discord = config.discord_mut().expect("discord provider");
    discord.api_base = "http://127.0.0.1:9".to_owned();
    let client = |discord: &vibe_talk::config::DiscordConfig| {
        HttpDiscordClient::new(discord)
            .expect("client")
            .supports_idempotent_posts()
    };
    assert!(
        !client(discord),
        "a direct client has no request id to hold a key in"
    );
    discord.channel_registration = true;
    discord.thread_api = ThreadApi::Native;
    assert!(
        !client(discord),
        "native thread posts go to the provider itself, which ignores the nonce"
    );
    discord.thread_api = ThreadApi::Bridge;
    assert!(client(discord));
    discord.thread_api = ThreadApi::Off;
    assert!(client(discord));
}
