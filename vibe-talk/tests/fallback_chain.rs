//! Read-aloud through an ordered fallback chain, at the HTTP boundary.
//!
//! `#563 read-aloud-fallback-chain`. The chain's per-provider behaviour
//! is covered by the unit tests in `src/chain.rs`; these tests cover
//! what only the assembled server shows: the configuration builds the
//! chain in order, a primary that refuses falls through to the next
//! voice for the same block, the answer names the voice that spoke, and
//! Settings is told the order.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::config::Config;
use vibe_talk::http::router;
use vibe_talk::model::ChannelId;
use vibe_talk::testing::{READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};

/// Configuration text for a chain deployment: `primary`, then `fallback`.
fn chain_toml(primary: &str, fallback: &[&str]) -> String {
    let fallback_list = fallback
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
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
backend = "{primary}"
fallback = [{fallback_list}]

[local_tts]
endpoint = "http://127.0.0.1:18771"
voice = "af_heart"
"#
    )
}

struct Harness {
    router: axum::Router,
    message: String,
}

fn harness(primary: &str, fallback: &[&str]) -> Harness {
    // No Wit.ai token is configured: the wit provider refuses every
    // block as not configured, which is the primary failure these tests
    // fall through from.
    let (state, discord, _elevenlabs) =
        vibe_talk::testing::state_from_toml(&chain_toml(primary, fallback));
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
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {READ_TOKEN}"));
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
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, value)
}

#[tokio::test]
async fn a_refusing_primary_falls_through_and_the_answer_names_the_voice() {
    let harness = harness("wit", &["local"]);
    let (status, prepared) = {
        let (status, _headers, value) = call(
            &harness,
            "POST",
            &format!("/api/v1/channels/{WRITE_CHANNEL}/speech/prepare"),
            Some(serde_json::json!({ "ids": [harness.message] })),
        )
        .await;
        (status, value)
    };
    assert_eq!(status, StatusCode::OK, "{prepared}");
    let url = prepared["prepared"][0]["url"]
        .as_str()
        .expect("ticket url")
        .to_owned();

    let mut builder = Request::builder().method("GET").uri(&url);
    builder = builder.header("authorization", format!("Bearer {READ_TOKEN}"));
    let response = harness
        .router
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("router responds");
    // The page asks with its ticket alone; this test reuses the read
    // token the same way the other speech tests do.
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-read-aloud-voice")
            .and_then(|value| value.to_str().ok()),
        Some("Local voice"),
        "the answer must name the voice that spoke"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert!(
        !vibe_talk::wit::speech::is_silent(&bytes),
        "audio was silence"
    );
}

#[tokio::test]
async fn client_config_names_the_chain_in_order() {
    let harness = harness("wit", &["local", "browser"]);
    let (status, _headers, payload) = call(&harness, "GET", "/api/v1/client-config", None).await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    assert_eq!(payload["read_aloud"]["backend"], "wit");
    assert_eq!(
        payload["read_aloud"]["chain"],
        serde_json::json!(["Wit.ai voice", "Local voice", "Device voice"])
    );
}

#[tokio::test]
async fn when_every_voice_refuses_the_last_refusal_is_the_answer() {
    // Browser refuses as a page-only voice; wit refuses as unconfigured.
    // The chain's answer is the LAST refusal, not the first.
    let harness = harness("browser", &["wit"]);
    let (status, _headers, payload) = call(
        &harness,
        "POST",
        &format!(
            "/api/v1/channels/{WRITE_CHANNEL}/messages/{}/speak",
            harness.message
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{payload}");
    assert_eq!(payload["error"], "wit_not_configured", "{payload}");
}

#[test]
fn a_chain_names_each_voice_once_with_the_device_last() {
    let base = chain_toml("local", &["wit"]);
    for (bad, why) in [
        (
            base.replace("fallback = [\"wit\"]", "fallback = [\"wit\", \"wit\"]"),
            "duplicate",
        ),
        (
            base.replace("fallback = [\"wit\"]", "fallback = [\"local\"]"),
            "primary repeated",
        ),
        (
            base.replace("fallback = [\"wit\"]", "fallback = [\"browser\", \"wit\"]"),
            "device not last",
        ),
    ] {
        let error =
            Config::from_toml_and_env(&bad, &std::collections::BTreeMap::new()).expect_err(why);
        assert!(
            error.to_string().contains("read_aloud.fallback"),
            "{why}: {error}"
        );
    }
    let ok = chain_toml("local", &["wit", "browser"]);
    let config =
        Config::from_toml_and_env(&ok, &std::collections::BTreeMap::new()).expect("valid chain");
    assert_eq!(
        config.read_aloud.fallback,
        vec![
            vibe_talk::config::ReadAloudBackend::Wit,
            vibe_talk::config::ReadAloudBackend::Browser,
        ]
    );
}
