//! Wit.ai read-aloud against a fake Wit.ai server on loopback.
//!
//! `#552 wit-tts-provider`. The fake server at the bottom of this file is
//! a real HTTP server speaking the public Wit.ai shapes — `POST
//! /synthesize?v=...` with a Bearer token and a JSON body, `GET
//! /voices?v=...` answering a locale map — so the REAL
//! [`vibe_talk::wit::http::HttpWitClient`] runs: its URLs, headers, body,
//! 429 backoff and error redaction are all exercised, and the adapter's
//! sentence chunking, cache and silence check run around it. The fake can
//! refuse the token, rate-limit, stall and answer silence, because a fake
//! that can only say yes would certify nothing.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::config::Config;
use vibe_talk::http::router;
use vibe_talk::model::ChannelId;
use vibe_talk::testing::{READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};

const TOKEN: &str = "wit-test-token-not-a-real-one";

/// What the fake Wit.ai server answers, when a test has chosen.
#[derive(Clone, Debug, Default, PartialEq)]
enum Mode {
    #[default]
    Speak,
    /// 429 for the next `remaining` synthesize calls, then speak.
    RateLimit(u32),
    /// 429 forever.
    RateLimitForever,
    /// Valid WAV in which every sample is zero.
    Silent,
    /// Wait, then speak.
    Slow(Duration),
}

/// One request the fake server saw, exactly as it arrived.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
    accept: Option<String>,
    body: String,
}

#[derive(Debug, Default)]
struct FakeWitServer {
    mode: Mutex<Mode>,
    seen: Mutex<Vec<Seen>>,
}

impl FakeWitServer {
    fn set(&self, mode: Mode) {
        *self.mode.lock().expect("mode") = mode;
    }
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("seen").clone()
    }
    fn synthesize_hits(&self) -> usize {
        self.seen()
            .iter()
            .filter(|seen| seen.path.starts_with("/synthesize"))
            .count()
    }
}

fn wav(samples: &[i16]) -> Vec<u8> {
    vibe_talk::wit::fake::fake_wav(samples)
}

/// Serve the fake Wit.ai API on a loopback port the kernel picks; returns
/// its base URL. One thread per connection, HTTP/1.0 close-per-response,
/// which reqwest handles without pooling assumptions.
fn serve_fake_wit(server: Arc<FakeWitServer>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake wit");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let server = Arc::clone(&server);
            std::thread::spawn(move || {
                let mut raw = Vec::new();
                let mut buf = [0_u8; 4096];
                let mut body_at: Option<usize> = None;
                let mut length = 0_usize;
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            raw.extend_from_slice(&buf[..n]);
                            if body_at.is_none() {
                                if let Some(at) = find(&raw, b"\r\n\r\n") {
                                    let head = String::from_utf8_lossy(&raw[..at]).into_owned();
                                    length = head
                                        .lines()
                                        .find_map(|line| {
                                            line.to_ascii_lowercase()
                                                .strip_prefix("content-length:")
                                                .and_then(|v| v.trim().parse::<usize>().ok())
                                        })
                                        .unwrap_or(0);
                                    body_at = Some(at + 4);
                                }
                            }
                            if body_at.is_some_and(|at| raw.len() >= at + length) {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let Some(at) = body_at else { return };
                let head = String::from_utf8_lossy(&raw[..at]).into_owned();
                let mut lines = head.lines();
                let request_line = lines.next().unwrap_or_default().to_owned();
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_owned();
                let path = parts.next().unwrap_or_default().to_owned();
                let header = |name: &str| {
                    head.lines().find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case(name)
                            .then(|| value.trim().to_owned())
                    })
                };
                let body = String::from_utf8_lossy(&raw[at..at + length]).into_owned();
                server.seen.lock().expect("seen").push(Seen {
                    method: method.clone(),
                    path: path.clone(),
                    authorization: header("authorization"),
                    accept: header("accept"),
                    body: body.clone(),
                });
                let (status, content_type, payload) = answer(&server, &method, &path, &header);
                let response = format!(
                    "HTTP/1.0 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
                    payload.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&payload);
                let _ = stream.flush();
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn answer(
    server: &FakeWitServer,
    method: &str,
    path: &str,
    header: &dyn Fn(&str) -> Option<String>,
) -> (u16, &'static str, Vec<u8>) {
    if header("authorization").as_deref() != Some(&format!("Bearer {TOKEN}")[..]) {
        // Echoing the presented token, as an upstream is free to: the
        // client's redaction is what keeps it out of the reader's error.
        let presented = header("authorization").unwrap_or_default();
        return (
            401,
            "application/json",
            format!(r#"{{"error":"the token {presented} is not valid"}}"#).into_bytes(),
        );
    }
    if method == "GET" && path.starts_with("/voices") {
        return (
            200,
            "application/json",
            br#"{"en":[{"name":"wit$Remi","gender":"male","styles":["default"]},{"name":"wit$Clara"}]}"#
                .to_vec(),
        );
    }
    if method == "POST" && path.starts_with("/synthesize") {
        let mode = server.mode.lock().expect("mode").clone();
        match mode {
            Mode::RateLimitForever => {
                return (429, "application/json", b"rate limited".to_vec());
            }
            Mode::RateLimit(remaining) if remaining > 0 => {
                server.set(Mode::RateLimit(remaining - 1));
                return (429, "application/json", b"rate limited".to_vec());
            }
            Mode::Silent => return (200, "audio/wav", wav(&[0; 1600])),
            Mode::Slow(delay) => std::thread::sleep(delay),
            _ => {}
        }
        return (200, "audio/wav", wav(&[800; 800]));
    }
    (404, "application/json", b"not found".to_vec())
}

/// Configuration text for a Wit.ai deployment against `api_base`, with
/// the token in a file the test wrote (never inline).
fn wit_toml(api_base: &str, token_file: &std::path::Path) -> String {
    format!(
        r#"
[server]
bind = "127.0.0.1:0"

[discord]
bot_token = "test-bot-token"

[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[[channels]]
id = "{READ_CHANNEL}"
label = "build noise"
writable = false

[[channels]]
id = "{WRITE_CHANNEL}"
label = "lead team"
writable = true

[read_aloud]
backend = "wit"

[wit]
token_file = "{}"
voice = "wit$Remi"
api_base = "{api_base}"
"#,
        token_file.display()
    )
}

fn token_file(contents: &str) -> std::path::PathBuf {
    // Unique per call: tests in one binary share a process, and two of
    // them writing one path would truncate each other's token mid-read.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "vibe-talk-wit-token-{}-{}.txt",
        std::process::id(),
        serial
    ));
    std::fs::write(&path, contents).expect("write token file");
    path
}

struct Harness {
    router: axum::Router,
    message: String,
}

fn harness(api_base: &str, token: &str) -> Harness {
    let file = token_file(token);
    let text = wit_toml(api_base, &file);
    let client = Arc::new(vibe_talk::wit::http::HttpWitClient::new(api_base).expect("wit client"));
    let (state, discord) = vibe_talk::testing::state_with_wit(&text, client);
    let message = discord
        .seed(
            &ChannelId(WRITE_CHANNEL.to_owned()),
            "codex-eng",
            "hello from the fake channel",
        )
        .0;
    Harness {
        router: router(state),
        message,
    }
}

async fn call(
    harness: &Harness,
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
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn call_bytes(
    harness: &Harness,
    uri: &str,
    token: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method("POST").uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = harness
        .router
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("router responds");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

#[tokio::test]
async fn a_message_is_read_through_the_fake_wit_server() {
    let server = Arc::new(FakeWitServer::default());
    let base = serve_fake_wit(Arc::clone(&server));
    let harness = harness(&base, TOKEN);

    let (status, headers, body) = call_bytes(
        &harness,
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/messages/{}/speak",
            harness.message
        ),
        Some(READ_TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("audio/wav")
    );
    assert!(
        !vibe_talk::wit::speech::is_silent(&body),
        "audio was silence"
    );

    let seen = server.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].method, "POST");
    assert!(
        seen[0].path.starts_with("/synthesize?v="),
        "{}",
        seen[0].path
    );
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert_eq!(seen[0].accept.as_deref(), Some("audio/wav"));
    let sent: Value = serde_json::from_str(&seen[0].body).expect("json body");
    assert_eq!(sent["voice"], "wit$Remi");
    assert!(
        sent["q"].as_str().is_some_and(|q| q.contains("hello")),
        "{sent}"
    );
}

#[tokio::test]
async fn a_refused_token_stops_the_read_and_never_reaches_an_error_page() {
    let server = Arc::new(FakeWitServer::default());
    let base = serve_fake_wit(Arc::clone(&server));
    let harness = harness(&base, "the-wrong-token");

    let (status, payload) = call(
        &harness,
        "POST",
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/messages/{}/speak",
            harness.message
        ),
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{payload}");
    assert_eq!(payload["error"], "wit_unauthorized", "{payload}");
    assert!(
        !payload.to_string().contains("the-wrong-token"),
        "the token reached the answer: {payload}"
    );
}

#[tokio::test]
async fn a_rate_limit_that_clears_is_retried_and_then_reads() {
    let server = Arc::new(FakeWitServer::default());
    server.set(Mode::RateLimit(2));
    let base = serve_fake_wit(Arc::clone(&server));
    let harness = harness(&base, TOKEN);

    let (status, _headers, body) = call_bytes(
        &harness,
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/messages/{}/speak",
            harness.message
        ),
        Some(READ_TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(server.synthesize_hits(), 3, "two 429s then the read");
}

#[tokio::test]
async fn a_rate_limit_that_never_clears_stops_the_read_with_its_own_message() {
    let server = Arc::new(FakeWitServer::default());
    server.set(Mode::RateLimitForever);
    let base = serve_fake_wit(Arc::clone(&server));
    let harness = harness(&base, TOKEN);

    let (status, payload) = call(
        &harness,
        "POST",
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/messages/{}/speak",
            harness.message
        ),
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{payload}");
    assert_eq!(payload["error"], "wit_rate_limited", "{payload}");
    assert_eq!(
        server.synthesize_hits(),
        vibe_talk::wit::http::MAX_ATTEMPTS as usize,
        "bounded retries, then the message"
    );
}

#[tokio::test]
async fn silent_audio_stops_the_read_instead_of_playing() {
    let server = Arc::new(FakeWitServer::default());
    server.set(Mode::Silent);
    let base = serve_fake_wit(Arc::clone(&server));
    let harness = harness(&base, TOKEN);

    let (status, payload) = call(
        &harness,
        "POST",
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/messages/{}/speak",
            harness.message
        ),
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{payload}");
    assert_eq!(payload["error"], "wit_silent_audio", "{payload}");
}

#[tokio::test]
async fn a_slow_wit_still_reads_and_an_absent_one_is_unreachable() {
    let server = Arc::new(FakeWitServer::default());
    server.set(Mode::Slow(Duration::from_millis(1200)));
    let base = serve_fake_wit(Arc::clone(&server));
    let harness = harness(&base, TOKEN);
    let (status, _headers, _body) = call_bytes(
        &harness,
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/messages/{}/speak",
            harness.message
        ),
        Some(READ_TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Nothing listening: the connection itself is refused.
    let dead = harness_like_dead();
    let (status, payload) = call(
        &dead,
        "POST",
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/messages/{}/speak",
            dead.message
        ),
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{payload}");
    assert_eq!(payload["error"], "wit_unreachable", "{payload}");
}

fn harness_like_dead() -> Harness {
    // Bind and drop a listener to learn a port nothing serves.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    harness(&format!("http://127.0.0.1:{port}"), TOKEN)
}

#[tokio::test]
async fn the_voices_route_lists_the_catalogue_for_settings() {
    let server = Arc::new(FakeWitServer::default());
    let base = serve_fake_wit(Arc::clone(&server));
    let harness = harness(&base, TOKEN);

    let (status, payload) = call(
        &harness,
        "GET",
        "/api/v1/read-aloud/voices",
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    assert_eq!(payload["backend"], "wit");
    let names: Vec<&str> = payload["voices"]
        .as_array()
        .expect("voices")
        .iter()
        .filter_map(|voice| voice["name"].as_str())
        .collect();
    assert_eq!(names, vec!["wit$Remi", "wit$Clara"]);
}

#[tokio::test]
async fn a_voice_chosen_in_settings_is_the_voice_asked_for() {
    let server = Arc::new(FakeWitServer::default());
    let base = serve_fake_wit(Arc::clone(&server));
    let harness = harness(&base, TOKEN);

    let (status, prepared) = call(
        &harness,
        "POST",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/speech/prepare"),
        Some(READ_TOKEN),
        Some(serde_json::json!({
            "ids": [harness.message],
            "voice": "wit$Clara",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prepared}");
    let url = prepared["prepared"][0]["url"]
        .as_str()
        .expect("url")
        .to_owned();

    // The ticket URL carries its own authority, as the real one does.
    let (status, payload) = call(&harness, "GET", &url, None, None).await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    let seen = server.seen();
    let synth: Vec<&Seen> = seen
        .iter()
        .filter(|seen| seen.path.starts_with("/synthesize"))
        .collect();
    assert_eq!(synth.len(), 1, "{seen:?}");
    let sent: Value = serde_json::from_str(&synth[0].body).expect("json body");
    assert_eq!(sent["voice"], "wit$Clara", "{sent}");
}

#[test]
fn an_inline_wit_token_is_refused_by_name() {
    let text = wit_toml("https://api.wit.ai", std::path::Path::new("/tmp/unused")).replace(
        "voice = \"wit$Remi\"",
        "voice = \"wit$Remi\"\ntoken = \"written-inline\"",
    );
    let error = Config::from_toml_and_env(&text, &std::collections::BTreeMap::new())
        .expect_err("inline token");
    assert!(error.to_string().contains("wit.token"), "{error}");
}

#[test]
fn a_named_token_file_that_cannot_be_read_stops_startup() {
    let text = wit_toml(
        "https://api.wit.ai",
        std::path::Path::new("/tmp/vibe-talk-wit-no-such-file.txt"),
    );
    let error = Config::from_toml_and_env(&text, &std::collections::BTreeMap::new())
        .expect_err("missing token file");
    assert!(error.to_string().contains("wit.token_file"), "{error}");
}

#[test]
fn a_missing_token_file_does_not_stop_a_deployment_that_reads_another_way() {
    let text = wit_toml(
        "https://api.wit.ai",
        std::path::Path::new("/tmp/vibe-talk-wit-no-such-file.txt"),
    )
    .replace("backend = \"wit\"", "backend = \"browser\"");
    let config =
        Config::from_toml_and_env(&text, &std::collections::BTreeMap::new()).expect("config");
    assert_eq!(
        config.read_aloud.backend,
        vibe_talk::config::ReadAloudBackend::Browser
    );
    assert!(config.wit.token.is_none());
}

#[test]
fn a_relative_token_file_is_refused() {
    let text = wit_toml(
        "https://api.wit.ai",
        std::path::Path::new("relative/token.txt"),
    );
    let error = Config::from_toml_and_env(&text, &std::collections::BTreeMap::new())
        .expect_err("relative token file");
    assert!(error.to_string().contains("wit.token_file"), "{error}");
}

#[test]
fn the_token_comes_from_the_file_and_wit_is_a_backend_choice() {
    let file = token_file(&format!("  {TOKEN}\n"));
    let text = wit_toml("https://api.wit.ai", &file);
    let config =
        Config::from_toml_and_env(&text, &std::collections::BTreeMap::new()).expect("config");
    assert_eq!(
        config.read_aloud.backend,
        vibe_talk::config::ReadAloudBackend::Wit
    );
    assert_eq!(
        config.wit.token.as_ref().map(|token| token.expose()),
        Some(TOKEN),
        "trimmed, from the file"
    );
    assert_eq!(config.wit.voice.as_deref(), Some("wit$Remi"));
}
