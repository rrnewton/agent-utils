//! Device speech selection and the provider-independent read-aloud boundary.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::config::{Config, ReadAloudBackend, ENV_READ_ALOUD_BACKEND};
use vibe_talk::http::router;
use vibe_talk::model::ChannelId;
use vibe_talk::speech::{Description, Playback, Speech, SpeechError, SpeechProvider};

#[tokio::test]
async fn conversational_read_aloud_drives_the_provider_neutral_socket_and_returns_wav() {
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accepts");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upgrades");
        socket
            .send(Message::Text(r#"{"type":"session_started"}"#.into()))
            .await
            .expect("starts");
        let prompt = socket.next().await.expect("prompt frame").expect("prompt");
        let Message::Text(prompt) = prompt else {
            panic!("prompt was not text")
        };
        assert!(prompt.contains("Read the channel message"));
        assert!(prompt.contains("hello from the channel"));
        socket
            .send(Message::Binary(vec![1, 2, 3, 4].into()))
            .await
            .expect("audio");
        socket
            .send(Message::Text(r#"{"type":"turn_complete","turn":1}"#.into()))
            .await
            .expect("finishes");
    });
    let config = vibe_talk::config::ConversationConfig {
        backend: vibe_talk::config::ConversationBackend::WebSocket,
        websocket_url: Some("ws://127.0.0.1:1/browser-route".to_owned()),
        label: "Test agent".to_owned(),
    };
    let server_route = format!("ws://{address}");
    let provider = vibe_talk::speech::ConversationSpeech::new(&config, Some(&server_route));
    assert_eq!(provider.describe().label, "Test agent");
    let spoken = provider
        .speak("hello from the channel", None)
        .await
        .expect("speaks");
    assert_eq!(spoken.content_type, "audio/wav");
    assert_eq!(&spoken.audio[..4], b"RIFF");
    assert_eq!(&spoken.audio[44..], &[1, 2, 3, 4]);
    server.await.expect("server exits");
}
use vibe_talk::testing::{self, READ_CHANNEL, READ_TOKEN};

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    payload: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {READ_TOKEN}"))
        .header("content-type", "application/json")
        .body(payload.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("router");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, bytes.to_vec())
}

#[test]
fn browser_selection_is_explicit_and_needs_no_elevenlabs_settings() {
    let base = testing::config_toml_without_elevenlabs();
    let default = Config::from_toml_and_env(&base, &BTreeMap::new()).expect("default config");
    assert_eq!(default.read_aloud.backend, ReadAloudBackend::ElevenLabs);

    let browser = format!("{base}\n[read_aloud]\nbackend = 'browser'\n");
    let config = Config::from_toml_and_env(&browser, &BTreeMap::new()).expect("browser config");
    assert_eq!(config.read_aloud.backend, ReadAloudBackend::Browser);
    assert!(config.elevenlabs.api_key.is_none());
    assert!(config.elevenlabs.agent_id.is_none());

    let overridden = Config::from_toml_and_env(
        &browser,
        &BTreeMap::from([(ENV_READ_ALOUD_BACKEND.to_owned(), "elevenlabs".to_owned())]),
    )
    .expect("environment selection");
    assert_eq!(overridden.read_aloud.backend, ReadAloudBackend::ElevenLabs);

    for invalid in ["cloud", "browsre", ""] {
        let text = format!("{base}\n[read_aloud]\nbackend = '{invalid}'\n");
        assert!(Config::from_toml_and_env(&text, &BTreeMap::new()).is_err());
    }
    assert!(Config::from_toml_and_env(
        &base,
        &BTreeMap::from([(ENV_READ_ALOUD_BACKEND.to_owned(), "cloud".to_owned())]),
    )
    .is_err());
}

#[tokio::test]
async fn browser_config_advertises_device_playback_and_never_uses_elevenlabs() {
    let text = format!(
        "{}\n[read_aloud]\nbackend = 'browser'\n",
        testing::config_toml_without_elevenlabs()
    );
    let (state, chat, elevenlabs) = testing::state_from_toml(&text);
    let id = chat.seed(
        &ChannelId(READ_CHANNEL.to_owned()),
        "Reader",
        "Read these words",
    );
    let app = router(state);
    let (status, bytes) = call(&app, "GET", "/api/v1/client-config", None).await;
    assert_eq!(status, StatusCode::OK);
    let config: Value = serde_json::from_slice(&bytes).expect("config");
    assert_eq!(
        config["read_aloud"],
        json!({"backend": "browser", "label": "Device voice", "playback": "browser", "local_only": true})
    );
    assert_eq!(config["elevenlabs_agent_id"], Value::Null);

    let (status, bytes) = call(
        &app,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/messages"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&bytes).contains("Read these words"));

    // A stale browser that still calls the audio routes receives an actionable answer, rather
    // than spending a vendor request or being told to configure credentials it does not need.
    for (uri, body) in [
        (
            format!("/api/v1/channels/{READ_CHANNEL}/messages/{id}/speak"),
            None,
        ),
        (
            format!("/api/v1/channels/{READ_CHANNEL}/speech/prepare"),
            Some(json!({"ids": [id.0]})),
        ),
    ] {
        let (status, bytes) = call(&app, "POST", &uri, body).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let error: Value = serde_json::from_slice(&bytes).expect("error");
        assert_eq!(error["error"], "browser_speech_required");
        assert!(!String::from_utf8_lossy(&bytes)
            .to_lowercase()
            .contains("elevenlabs"));
    }
    assert!(elevenlabs.requested().is_empty());
    assert!(elevenlabs.spoken().is_empty());
    assert!(elevenlabs.chats().is_empty());
}

struct IndependentSpeech;

#[async_trait::async_trait]
impl SpeechProvider for IndependentSpeech {
    fn describe(&self) -> Description {
        Description {
            backend: "independent",
            label: "Independent voice".to_owned(),
            playback: Playback::Audio,
            local_only: false,
        }
    }

    async fn speak(&self, text: &str, _speed: Option<f64>) -> Result<Speech, SpeechError> {
        assert!(
            !text.is_empty(),
            "warming a provider must not synthesize empty text"
        );
        Ok(Speech {
            audio: text.as_bytes().to_vec(),
            content_type: "audio/test".to_owned(),
        })
    }
}

#[tokio::test]
async fn audio_routes_accept_a_provider_that_has_no_elevenlabs_configuration() {
    let (mut state, chat, elevenlabs) =
        testing::state_from_toml(&testing::config_toml_without_elevenlabs());
    state.speech = Arc::new(IndependentSpeech);
    let id = chat.seed(
        &ChannelId(READ_CHANNEL.to_owned()),
        "Reader",
        "**Speak** this",
    );
    let app = router(state);
    let (status, bytes) = call(&app, "GET", "/api/v1/client-config", None).await;
    assert_eq!(status, StatusCode::OK);
    let config: Value = serde_json::from_slice(&bytes).expect("config");
    assert_eq!(config["read_aloud"]["backend"], "independent");
    assert_eq!(config["read_aloud"]["playback"], "audio");

    let (status, bytes) = call(
        &app,
        "POST",
        &format!("/api/v1/channels/{READ_CHANNEL}/messages/{id}/speak"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(String::from_utf8(bytes).expect("test audio"), "Speak this");

    let (status, bytes) = call(
        &app,
        "POST",
        &format!("/api/v1/channels/{READ_CHANNEL}/speech/prepare"),
        Some(json!({"ids": [id.0]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let prepared: Value = serde_json::from_slice(&bytes).expect("prepared");
    let url = prepared["prepared"][0]["url"].as_str().expect("ticket");
    let (status, bytes) = call(&app, "GET", url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        String::from_utf8(bytes).expect("stream audio"),
        "Speak this"
    );
    assert!(elevenlabs.requested().is_empty());
    assert!(elevenlabs.spoken().is_empty());
}

#[tokio::test]
async fn elevenlabs_audio_failures_keep_their_codes_status_and_redaction() {
    const REJECTED_KEY: &str = "xi-rejected-test-key";
    let config =
        testing::config_toml().replace(vibe_talk::elevenlabs::fake::VALID_API_KEY, REJECTED_KEY);
    let (state, chat, _elevenlabs) = testing::state_from_toml(&config);
    let id = chat.seed(
        &ChannelId(READ_CHANNEL.to_owned()),
        "Reader",
        "Read these words",
    );
    let app = router(state);
    let (status, bytes) = call(&app, "GET", "/api/v1/client-config", None).await;
    assert_eq!(status, StatusCode::OK);
    let config: Value = serde_json::from_slice(&bytes).expect("config");
    assert_eq!(config["read_aloud"]["backend"], "elevenlabs");
    assert_eq!(config["read_aloud"]["playback"], "audio");

    let (status, bytes) = call(
        &app,
        "POST",
        &format!("/api/v1/channels/{READ_CHANNEL}/speech/prepare"),
        Some(json!({"ids": [id.0]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let prepared: Value = serde_json::from_slice(&bytes).expect("prepared");
    let url = prepared["prepared"][0]["url"].as_str().expect("ticket");

    for (method, uri) in [
        ("GET", url.to_owned()),
        (
            "POST",
            format!("/api/v1/channels/{READ_CHANNEL}/messages/{id}/speak"),
        ),
    ] {
        let (status, bytes) = call(&app, method, &uri, None).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        let error: Value = serde_json::from_slice(&bytes).expect("error");
        assert_eq!(error["error"], "elevenlabs_error");
        assert!(error["detail"]
            .as_str()
            .expect("detail")
            .contains("HTTP 401"));
        assert!(!String::from_utf8_lossy(&bytes).contains(REJECTED_KEY));
    }
}
