//! Local read-aloud against a fake sidecar server on loopback.
//!
//! `#561 local-tts-provider`. The fake server at the bottom of this file
//! speaks the sidecar contract — `POST /synthesize` with a JSON body,
//! `GET /voices` answering a catalogue — so the REAL
//! [`vibe_talk::local_tts::http::HttpLocalTtsClient`] runs: its URLs,
//! body and error mapping are all exercised, and the adapter's sentence
//! chunking, cache and silence check run around it. The fake can stall,
//! refuse and answer silence, because a fake that can only say yes would
//! certify nothing. The real Kokoro sidecar is exercised separately (it
//! needs model weights): `scripts/local_tts_sidecar.py`, measured in the
//! issue's notes.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::http::router;
use vibe_talk::model::ChannelId;
use vibe_talk::testing::{READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};

/// What the fake sidecar answers, when a test has chosen.
#[derive(Clone, Debug, Default, PartialEq)]
enum Mode {
    #[default]
    Speak,
    /// Valid WAV in which every sample is zero.
    Silent,
    /// A 500 with an explanation, as a crashed model would surface.
    Refuse,
    /// Wait, then speak.
    Slow(Duration),
}

/// One request the fake sidecar saw, exactly as it arrived.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    body: String,
}

#[derive(Debug, Default)]
struct FakeSidecar {
    mode: Mutex<Mode>,
    seen: Mutex<Vec<Seen>>,
}

impl FakeSidecar {
    fn set(&self, mode: Mode) {
        *self.mode.lock().expect("mode") = mode;
    }
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("seen").clone()
    }
}

/// Serve the fake sidecar on a loopback port the kernel picks; returns
/// its base URL. One thread per connection, HTTP/1.0 close-per-response.
fn serve_fake_sidecar(server: Arc<FakeSidecar>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake sidecar");
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
                let body = String::from_utf8_lossy(&raw[at..at + length]).into_owned();
                server.seen.lock().expect("seen").push(Seen {
                    method: method.clone(),
                    path: path.clone(),
                    body: body.clone(),
                });
                let (status, content_type, payload) = answer(&server, &method, &path);
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

fn answer(server: &FakeSidecar, method: &str, path: &str) -> (u16, &'static str, Vec<u8>) {
    if method == "GET" && path.starts_with("/voices") {
        return (
            200,
            "application/json",
            br#"{"voices":[{"name":"af_heart","locale":"en"},{"name":"am_michael","locale":"en"}]}"#
                .to_vec(),
        );
    }
    if method == "POST" && path.starts_with("/synthesize") {
        match server.mode.lock().expect("mode").clone() {
            Mode::Refuse => return (500, "application/json", b"model crashed".to_vec()),
            Mode::Silent => {
                return (200, "audio/wav", vibe_talk::wit::fake::fake_wav(&[0; 1600]));
            }
            Mode::Slow(delay) => std::thread::sleep(delay),
            Mode::Speak => {}
        }
        return (
            200,
            "audio/wav",
            vibe_talk::wit::fake::fake_wav(&[800; 800]),
        );
    }
    (404, "application/json", b"not found".to_vec())
}

/// Configuration text for a local deployment against `endpoint`.
fn local_toml(endpoint: &str) -> String {
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
backend = "local"

[local_tts]
endpoint = "{endpoint}"
voice = "af_heart"
"#
    )
}

struct Harness {
    router: axum::Router,
    message: String,
}

fn harness(endpoint: &str) -> Harness {
    let text = local_toml(endpoint);
    let client = Arc::new(
        vibe_talk::local_tts::http::HttpLocalTtsClient::new(endpoint).expect("local client"),
    );
    let (state, discord) = vibe_talk::testing::state_with_local(&text, client);
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
async fn a_message_is_read_through_the_fake_sidecar() {
    let server = Arc::new(FakeSidecar::default());
    let base = serve_fake_sidecar(Arc::clone(&server));
    let harness = harness(&base);

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
    assert_eq!(seen[0].path, "/synthesize");
    let sent: Value = serde_json::from_str(&seen[0].body).expect("json body");
    assert_eq!(sent["voice"], "af_heart");
    assert!(
        sent["q"].as_str().is_some_and(|q| q.contains("hello")),
        "{sent}"
    );
}

#[tokio::test]
async fn a_down_sidecar_and_a_refusing_one_stop_the_read_differently() {
    let server = Arc::new(FakeSidecar::default());
    server.set(Mode::Refuse);
    let base = serve_fake_sidecar(Arc::clone(&server));
    let refusing = harness(&base);
    let (status, payload) = call(
        &refusing,
        "POST",
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/messages/{}/speak",
            refusing.message
        ),
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{payload}");
    assert_eq!(payload["error"], "local_tts_error", "{payload}");

    // Nothing listening: the connection itself is refused.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let dead = harness(&format!("http://127.0.0.1:{port}"));
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
    assert_eq!(payload["error"], "local_tts_unreachable", "{payload}");
}

#[tokio::test]
async fn silent_audio_stops_the_read_instead_of_playing() {
    let server = Arc::new(FakeSidecar::default());
    server.set(Mode::Silent);
    let base = serve_fake_sidecar(Arc::clone(&server));
    let harness = harness(&base);

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
    assert_eq!(payload["error"], "local_tts_silent_audio", "{payload}");
}

#[tokio::test]
async fn a_slow_sidecar_still_reads_and_the_voices_route_lists_its_catalogue() {
    let server = Arc::new(FakeSidecar::default());
    server.set(Mode::Slow(Duration::from_millis(800)));
    let base = serve_fake_sidecar(Arc::clone(&server));
    let harness = harness(&base);
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

    let (status, payload) = call(
        &harness,
        "GET",
        "/api/v1/read-aloud/voices",
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    assert_eq!(payload["backend"], "local");
    let names: Vec<&str> = payload["voices"]
        .as_array()
        .expect("voices")
        .iter()
        .filter_map(|voice| voice["name"].as_str())
        .collect();
    assert_eq!(names, vec!["af_heart", "am_michael"]);
}
