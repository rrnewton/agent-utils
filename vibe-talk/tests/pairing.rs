//! `#369 pairing-link`: a single-use, short-lived code that a phone trades for the write token.
//!
//! The expiry boundary is pinned in `src/pairing.rs`, where the clock can be handed in; here every
//! claim is made through the router, as a phone and an agent reach it, and through the real
//! `vibe-talk pair-link` binary against a real listener. That nothing secret reaches the log is
//! in `tests/logging.rs`, where every test captures it.

use std::process::Command;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::http::router;
use vibe_talk::testing::{READ_TOKEN, WRITE_TOKEN};

async fn call(
    router: &axum::Router,
    uri: &str,
    token: Option<&str>,
    body: &str,
) -> (StatusCode, Value, axum::http::HeaderMap) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.to_owned())).expect("request"))
        .await
        .expect("router responds");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        headers,
    )
}

async fn mint(router: &axum::Router, body: &str) -> String {
    let (status, answer, _) = call(router, "/api/v1/pair", Some(WRITE_TOKEN), body).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer["code"].as_str().expect("a code").to_owned()
}

async fn redeem(router: &axum::Router, code: &str) -> (StatusCode, Value) {
    let (status, answer, _) = call(
        router,
        "/api/v1/pair/redeem",
        None,
        &json!({ "code": code }).to_string(),
    )
    .await;
    (status, answer)
}

#[tokio::test]
async fn a_minted_code_redeems_once_for_the_write_token() {
    let (state, _discord) = vibe_talk::testing::state();
    let router = router(state);
    let (status, answer, headers) = call(&router, "/api/v1/pair", Some(WRITE_TOKEN), "{}").await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let code = answer["code"].as_str().expect("code");
    assert_eq!(code.len(), 43, "256 bits, base64url");
    let expires = answer["expires_at"]
        .as_str()
        .expect("expires_at")
        .parse::<jiff::Timestamp>()
        .expect("RFC 3339");
    let ahead = expires.duration_since(jiff::Timestamp::now()).as_secs();
    assert!(
        (295..=300).contains(&ahead),
        "default ttl is 300 s: {ahead}"
    );
    assert_eq!(headers["cache-control"], "no-store");

    let (status, answer, headers) = call(
        &router,
        "/api/v1/pair/redeem",
        None,
        &json!({ "code": code }).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer, json!({ "token": WRITE_TOKEN }));
    assert_eq!(headers["cache-control"], "no-store");

    let (status, answer) = redeem(&router, code).await;
    assert_eq!(status, StatusCode::GONE, "a used code is gone: {answer}");
    assert_eq!(answer["error"], "pairing_link_invalid");
}

#[tokio::test]
async fn several_codes_may_be_live_and_each_is_its_own() {
    let (state, _discord) = vibe_talk::testing::state();
    let router = router(state);
    let first = mint(&router, "{}").await;
    let second = mint(&router, r#"{"ttl_seconds": 60}"#).await;
    assert_ne!(first, second);
    assert_eq!(redeem(&router, &second).await.0, StatusCode::OK);
    assert_eq!(redeem(&router, &first).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_ttl_outside_one_to_six_hundred_seconds_is_refused() {
    let (state, _discord) = vibe_talk::testing::state();
    let router = router(state);
    for body in [r#"{"ttl_seconds": 0}"#, r#"{"ttl_seconds": 601}"#] {
        let (status, answer, _) = call(&router, "/api/v1/pair", Some(WRITE_TOKEN), body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
        assert_eq!(answer["error"], "invalid_ttl");
    }
    mint(&router, r#"{"ttl_seconds": 600}"#).await;
}

#[tokio::test]
async fn the_read_token_cannot_mint_and_nothing_else_can_either() {
    let (state, _discord) = vibe_talk::testing::state();
    let pairing = std::sync::Arc::clone(&state.pairing);
    let router = router(state);
    let (status, _, _) = call(&router, "/api/v1/pair", Some(READ_TOKEN), "{}").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the read token would become the write token"
    );
    let (status, _, _) = call(&router, "/api/v1/pair", None, "{}").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(pairing.live(), 0, "a refused caller minted a code");
}

#[tokio::test]
async fn a_wrong_or_malformed_code_is_refused_with_the_same_answer() {
    let (state, _discord) = vibe_talk::testing::state();
    let router = router(state);
    let code = mint(&router, "{}").await;
    let (wrong_status, wrong, _) = call(
        &router,
        "/api/v1/pair/redeem",
        None,
        &json!({ "code": "A".repeat(43) }).to_string(),
    )
    .await;
    assert_eq!(wrong_status, StatusCode::GONE);
    assert!(
        wrong["detail"]
            .as_str()
            .is_some_and(|d| d.contains("expired or was already used")),
        "{wrong}"
    );
    for body in ["", "not json", "{}", r#"{"code": 7}"#, &"x".repeat(4096)] {
        let (status, answer, _) = call(&router, "/api/v1/pair/redeem", None, body).await;
        assert_eq!((status, &answer), (wrong_status, &wrong), "{body:.20}");
    }
    // A bearer token does not stand in for the code.
    let (status, _, _) = call(&router, "/api/v1/pair/redeem", Some(WRITE_TOKEN), "{}").await;
    assert_eq!(status, StatusCode::GONE);
    // None of that burned the real code.
    assert_eq!(redeem(&router, &code).await.0, StatusCode::OK);
}

#[tokio::test]
async fn repeated_failures_are_answered_429_and_the_right_code_still_works() {
    let (state, _discord) = vibe_talk::testing::state();
    let router = router(state);
    let code = mint(&router, "{}").await;
    for attempt in 0..vibe_talk::pairing::FAILURE_LIMIT {
        assert_eq!(
            redeem(&router, "guess").await.0,
            StatusCode::GONE,
            "attempt {attempt}"
        );
    }
    let (status, answer, headers) = call(
        &router,
        "/api/v1/pair/redeem",
        None,
        &json!({ "code": "guess" }).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{answer}");
    assert_eq!(headers["retry-after"], "60");
    // Behind a reverse proxy this address is everyone's: the person's own link still works.
    let (status, answer) = redeem(&router, &code).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
}

// --- the CLI, against a real listener --------------------------------------------------------

/// A configuration naming `bind` and, when given, a public URL.
fn config_file(
    name: &str,
    bind: std::net::SocketAddr,
    public_url: Option<&str>,
) -> std::path::PathBuf {
    let text = vibe_talk::testing::config_toml().replace(
        "bind = \"127.0.0.1:0\"",
        &match public_url {
            Some(url) => format!("bind = \"{bind}\"\npublic_url = \"{url}\""),
            None => format!("bind = \"{bind}\""),
        },
    );
    let dir =
        std::env::temp_dir().join(format!("vibe-talk-pair-link-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("vibe-talk.toml");
    std::fs::write(&path, text).expect("write config");
    path
}

/// Serve a test state on a loopback port, with the peer address as the server has it.
async fn serve() -> (
    std::net::SocketAddr,
    std::sync::Arc<vibe_talk::pairing::Pairing>,
    axum::Router,
) {
    let (state, _discord) = vibe_talk::testing::state();
    let pairing = std::sync::Arc::clone(&state.pairing);
    let app = router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let served = app.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            served.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("serve");
    });
    (address, pairing, app)
}

/// Run `vibe-talk pair-link` with no ambient `VIBE_TALK_*` setting, returning status, stdout and
/// stderr.
async fn pair_link(args: Vec<String>) -> (std::process::ExitStatus, String, String) {
    tokio::task::spawn_blocking(move || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vibe-talk"));
        command.arg("pair-link").args(&args);
        for (key, _) in std::env::vars() {
            if key.starts_with("VIBE_TALK_") || key.to_ascii_lowercase().ends_with("_proxy") {
                command.env_remove(key);
            }
        }
        let output = command.output().expect("running the binary");
        (
            output.status,
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    })
    .await
    .expect("join")
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_link_prints_a_working_link_and_its_expiry_and_never_the_token() {
    let (address, pairing, app) = serve().await;
    let config = config_file("prints", address, Some("https://vibe.example/"));
    let (status, stdout, stderr) = pair_link(vec![
        "--config".into(),
        config.display().to_string(),
        "--ttl".into(),
        "120".into(),
    ])
    .await;
    assert!(status.success(), "{stdout}{stderr}");
    for secret in [WRITE_TOKEN, READ_TOKEN] {
        assert!(
            !stdout.contains(&secret[..12]) && !stderr.contains(&secret[..12]),
            "pair-link printed a token: {stdout}{stderr}"
        );
    }
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "the link and its expiry, nothing else: {stdout}"
    );
    let code = lines[0]
        .strip_prefix("https://vibe.example/#pair=")
        .unwrap_or_else(|| panic!("not a pairing link on the public URL: {stdout}"));
    assert_eq!(code.len(), 43);
    let expires = lines[1]
        .strip_prefix("expires ")
        .expect("an expiry line")
        .parse::<jiff::Timestamp>()
        .expect("RFC 3339");
    let ahead = expires.duration_since(jiff::Timestamp::now()).as_secs();
    assert!((100..=120).contains(&ahead), "--ttl 120: {ahead}");
    assert!(
        stderr.contains("Send this link only where the person will read it"),
        "no reminder of where it is safe to send: {stderr}"
    );
    assert_eq!(pairing.live(), 1);
    // And the link works, once.
    assert_eq!(redeem(&app, code).await.0, StatusCode::OK);
    assert_eq!(redeem(&app, code).await.0, StatusCode::GONE);
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_link_takes_base_url_and_refuses_without_one() {
    let (address, pairing, _app) = serve().await;
    let config = config_file("bare", address, None);
    let (status, stdout, stderr) =
        pair_link(vec!["--config".into(), config.display().to_string()]).await;
    assert!(!status.success());
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("public_url"), "{stderr}");
    assert_eq!(
        pairing.live(),
        0,
        "a code was minted with nowhere to send it"
    );

    let (status, stdout, stderr) = pair_link(vec![
        "--config".into(),
        config.display().to_string(),
        "--base-url".into(),
        "http://phone.example:8080".into(),
    ])
    .await;
    assert!(status.success(), "{stdout}{stderr}");
    assert!(
        stdout.starts_with("http://phone.example:8080/#pair="),
        "{stdout}"
    );
    assert!(
        !stdout.contains("?"),
        "the code must never be in a query string: {stdout}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_link_says_when_the_server_is_not_running() {
    // A port nothing listens on: bind one, note it, and let it go.
    let address = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address");
    let config = config_file("down", address, Some("https://vibe.example"));
    let (status, stdout, stderr) =
        pair_link(vec!["--config".into(), config.display().to_string()]).await;
    assert!(!status.success());
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("is it running"), "{stderr}");
    assert!(!stderr.contains(&WRITE_TOKEN[..12]), "{stderr}");
}

// --- a link pasted into a bridged channel ------------------------------------------------------

/// GET `uri` with `token`, and the whole body as text.
async fn read_text(router: &axum::Router, uri: &str, token: &str) -> String {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn a_link_in_a_bridged_channel_never_reaches_a_read_scope_caller() {
    let (state, discord) = vibe_talk::testing::state();
    let live = std::sync::Arc::clone(&state.live);
    let router = router(state);
    let code = mint(&router, "{}").await;
    let channel = vibe_talk::model::ChannelId(vibe_talk::testing::READ_CHANNEL.to_owned());
    let mut cursor = None;
    vibe_talk::live::poll_once(discord.as_ref(), live.as_ref(), &channel, 50, &mut cursor)
        .await
        .expect("seed");
    // A live stream, opened by the read token before the link is posted.
    let mut stream = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/channels/{}/stream", channel.0))
                .header("authorization", format!("Bearer {READ_TOKEN}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router responds")
        .into_body();
    discord.seed(
        &channel,
        "coordinator",
        &format!("here is your link: https://vibe.example/#pair={code} (five minutes)"),
    );
    vibe_talk::live::poll_once(discord.as_ref(), live.as_ref(), &channel, 50, &mut cursor)
        .await
        .expect("publish");
    let mut streamed = String::new();
    while !streamed.contains("five minutes") {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), stream.frame())
            .await
            .unwrap_or_else(|_| panic!("the stream sent nothing: {streamed:?}"))
            .expect("the stream ended")
            .expect("frame");
        if let Some(data) = frame.data_ref() {
            streamed.push_str(&String::from_utf8_lossy(data));
        }
    }

    let messages = format!("/api/v1/channels/{}/messages?limit=5", channel.0);
    let as_reader = read_text(&router, &messages, READ_TOKEN).await;
    let as_owner = read_text(&router, &messages, WRITE_TOKEN).await;
    let mcp = {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("authorization", format!("Bearer {READ_TOKEN}"))
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(
                        json!({
                            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                            "params": { "name": "read_page",
                                        "arguments": { "channel_id": channel.0 } },
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("router responds");
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    for (what, text) in [
        ("the stream", &streamed),
        ("a read", &as_reader),
        ("an MCP tool", &mcp),
    ] {
        assert!(
            text.contains("here is your link"),
            "{what} did not carry the message at all, so this proves nothing: {text}"
        );
        assert!(
            !text.contains(&code[..12]),
            "{what} handed a read-scope caller the code: {text}"
        );
        assert!(text.contains("#pair=[redacted]"), "{what}: {text}");
    }
    assert!(
        as_owner.contains(&code),
        "the write token's own reader lost the link: {as_owner}"
    );
    // And the code is still live for the person it was sent to.
    assert_eq!(redeem(&router, &code).await.0, StatusCode::OK);
}
