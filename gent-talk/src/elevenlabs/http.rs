//! The real ElevenLabs client.
//!
//! Same shape as [`crate::discord::http`]: the parts with judgment in them — which URL, which
//! header, and how the answer maps onto a [`SignedUrl`] — are pure functions, unit-tested here.
//! The async wrapper around them is deliberately thin, because it is the only part that cannot be
//! exercised without a live account key.

use async_trait::async_trait;

use super::{
    credentials, speech_credentials, SignedUrl, SignedUrlError, SignedUrlProvider, Speech,
    SpeechError, SpeechProvider,
};
use crate::config::{ElevenLabsConfig, Secret};

/// A request, described independently of any HTTP client.
#[derive(Debug, PartialEq, Eq)]
pub struct PreparedRequest {
    /// HTTP method.
    pub method: &'static str,
    /// Fully qualified URL. Carries the agent id, which is public; never the key.
    pub url: String,
}

/// The header ElevenLabs authenticates with. Not `Authorization`, and not a bearer token.
pub const API_KEY_HEADER: &str = "xi-api-key";

/// Build the request that mints a signed conversation URL.
///
/// Documented as `GET /v1/convai/conversation/get-signed-url?agent_id=<id>` with the account key
/// in the `xi-api-key` header. The key deliberately does NOT go in the query string: a URL is the
/// single most-logged, most-forwarded, most-referrer-leaked part of an HTTP request.
#[must_use]
pub fn signed_url_request(api_base: &str, agent_id: &str) -> PreparedRequest {
    PreparedRequest {
        method: "GET",
        url: format!(
            "{}/convai/conversation/get-signed-url?agent_id={}",
            api_base.trim_end_matches('/'),
            urlencode(agent_id)
        ),
    }
}

/// The default ElevenLabs model for reading a message aloud.
///
/// Named here rather than left to the vendor's account default, so two deployments of this server
/// read the same text the same way and a change of model is a change to this line.
pub const TTS_MODEL: &str = "eleven_turbo_v2_5";

/// The audio this server asks for, and therefore what it tells the browser it is sending.
pub const TTS_CONTENT_TYPE: &str = "audio/mpeg";

/// Build the request that reads text aloud.
///
/// Documented as `POST /v1/text-to-speech/{voice_id}` with the account key in the `xi-api-key`
/// header. A POST rather than a GET even though it only reads: it SPENDS MONEY at a vendor, and a
/// GET is fair game for a browser, a proxy or a crawler to fetch speculatively.
#[must_use]
pub fn speech_request(api_base: &str, voice_id: &str) -> PreparedRequest {
    PreparedRequest {
        method: "POST",
        url: format!(
            "{}/text-to-speech/{}",
            api_base.trim_end_matches('/'),
            urlencode(voice_id)
        ),
    }
}

/// The body that goes with [`speech_request`].
#[must_use]
pub fn speech_body(text: &str) -> serde_json::Value {
    serde_json::json!({ "text": text, "model_id": TTS_MODEL })
}

/// Percent-encode the few characters that could change the meaning of a query string.
///
/// An agent id is `agent_` plus hex in practice, so this is a guard rather than a general encoder;
/// it exists so a pasted-with-junk id fails ElevenLabs' validation rather than smuggling a second
/// query parameter into the URL.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Convert ElevenLabs' answer into a [`SignedUrl`].
///
/// # Errors
///
/// Returns [`SignedUrlError::Shape`] when `signed_url` is absent, not a string, or blank. A blank
/// one is rejected rather than passed through, because it would surface as an inscrutable browser
/// failure several steps later.
pub fn parse_signed_url(
    value: &serde_json::Value,
    agent_id: &str,
) -> Result<SignedUrl, SignedUrlError> {
    let signed = value
        .get("signed_url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            SignedUrlError::Shape("answer has no string \"signed_url\" field".to_owned())
        })?;
    if signed.trim().is_empty() {
        return Err(SignedUrlError::Shape(
            "\"signed_url\" is blank; there is nothing to connect to".to_owned(),
        ));
    }
    Ok(SignedUrl {
        signed_url: signed.to_owned(),
        agent_id: agent_id.to_owned(),
        valid_for_seconds: super::DOCUMENTED_VALIDITY_SECONDS,
    })
}

/// A live ElevenLabs client.
#[derive(Debug)]
pub struct HttpElevenLabsClient {
    client: reqwest::Client,
}

impl HttpElevenLabsClient {
    /// Build a client.
    ///
    /// # Errors
    ///
    /// Returns [`SignedUrlError::Transport`] when the underlying HTTP client cannot be built.
    pub fn new() -> Result<Self, SignedUrlError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "gent-talk (https://github.com/rrnewton/agent-utils, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| SignedUrlError::Transport(e.to_string()))?;
        Ok(Self { client })
    }

    async fn get(
        &self,
        request: &PreparedRequest,
        api_key: &Secret,
    ) -> Result<serde_json::Value, SignedUrlError> {
        let response = self
            .client
            .get(&request.url)
            .header(API_KEY_HEADER, api_key.expose())
            .send()
            .await
            // A transport error's text can contain the URL, which holds the agent id but never the
            // key. Redact anyway: this is the cheap end of "never leak", and reqwest is free to
            // include whatever it likes here.
            .map_err(|e| SignedUrlError::Transport(super::redact(&e.to_string(), api_key)))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| SignedUrlError::Transport(super::redact(&e.to_string(), api_key)))?;
        if !status.is_success() {
            return Err(SignedUrlError::from_response(
                status.as_u16(),
                &text,
                api_key,
            ));
        }
        serde_json::from_str(&text)
            .map_err(|e| SignedUrlError::Shape(super::redact(&e.to_string(), api_key)))
    }
}

#[async_trait]
impl SignedUrlProvider for HttpElevenLabsClient {
    async fn signed_url(&self, config: &ElevenLabsConfig) -> Result<SignedUrl, SignedUrlError> {
        let (agent_id, api_key) = credentials(config)?;
        let request = signed_url_request(&config.api_base, agent_id);
        let value = self.get(&request, api_key).await?;
        parse_signed_url(&value, agent_id)
    }
}

#[async_trait]
impl SpeechProvider for HttpElevenLabsClient {
    async fn speak(&self, config: &ElevenLabsConfig, text: &str) -> Result<Speech, SpeechError> {
        let (voice_id, api_key) = speech_credentials(config)?;
        // Checked BEFORE the call, not after: a blank message would otherwise be billed as a
        // request that returns silence.
        if text.trim().is_empty() {
            return Err(SpeechError::Empty);
        }
        let request = speech_request(&config.api_base, voice_id);
        let response = self
            .client
            .post(&request.url)
            .header(API_KEY_HEADER, api_key.expose())
            .json(&speech_body(text))
            .send()
            .await
            .map_err(|e| SpeechError::Transport(super::redact(&e.to_string(), api_key)))?;
        let status = response.status();
        if !status.is_success() {
            // The FAILURE body is text; the success body is audio. Only this branch decodes as a
            // string, because calling `.text()` on megabytes of mp3 would be nonsense.
            let body = response.text().await.unwrap_or_default();
            return Err(SpeechError::from_response(status.as_u16(), &body, api_key));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or(TTS_CONTENT_TYPE)
            .to_owned();
        let audio = response
            .bytes()
            .await
            .map_err(|e| SpeechError::Transport(super::redact(&e.to_string(), api_key)))?;
        Ok(Speech {
            audio: audio.to_vec(),
            content_type,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_ELEVENLABS_API_BASE;

    #[test]
    fn the_url_matches_the_documented_endpoint() {
        let request = signed_url_request(DEFAULT_ELEVENLABS_API_BASE, "agent_0123");
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.url,
            "https://api.elevenlabs.io/v1/convai/conversation/get-signed-url?agent_id=agent_0123"
        );
    }

    #[test]
    fn the_url_tolerates_a_trailing_slash_on_the_base() {
        let request = signed_url_request("https://example.test/v1/", "agent_1");
        assert_eq!(
            request.url,
            "https://example.test/v1/convai/conversation/get-signed-url?agent_id=agent_1"
        );
    }

    #[test]
    fn the_url_never_carries_the_key() {
        // The whole reason the key goes in a header. If this ever regresses, the credential is in
        // every proxy log between here and ElevenLabs.
        let request = signed_url_request(DEFAULT_ELEVENLABS_API_BASE, "agent_1");
        assert!(!request.url.contains("api_key"), "{}", request.url);
        assert!(!request.url.contains("xi-"), "{}", request.url);
    }

    #[test]
    fn a_junk_agent_id_cannot_smuggle_a_second_parameter() {
        let request = signed_url_request(DEFAULT_ELEVENLABS_API_BASE, "agent_1&admin=true");
        assert!(
            request.url.ends_with("agent_id=agent_1%26admin%3Dtrue"),
            "{}",
            request.url
        );
    }

    /// A one-shot HTTP server on loopback that captures the request head and answers with
    /// `status` and `body`. It exists so the live client's *transport* choices — which header
    /// carries the key, and what happens to a rejection body — are checked by execution rather
    /// than by reading the code. Without it, moving the key into the query string is a change no
    /// test notices.
    async fn one_shot_server(
        status: &'static str,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            let (mut stream, _peer) = listener.accept().await.expect("accept");
            let mut buffer = vec![0_u8; 8192];
            let read = stream.read(&mut buffer).await.expect("read request");
            let head = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
            stream.flush().await.expect("flush");
            head
        });
        (format!("http://{addr}/v1"), handle)
    }

    fn live_config(api_base: String) -> ElevenLabsConfig {
        ElevenLabsConfig {
            agent_id: Some("agent_live".to_owned()),
            api_key: Some(Secret::new("xi-live-key-value")),
            api_base,
            voice_id: Some("voice_live".to_owned()),
        }
    }

    #[tokio::test]
    async fn the_live_client_sends_the_key_as_a_header_and_never_in_the_url() {
        let (api_base, server) =
            one_shot_server("200 OK", r#"{"signed_url":"wss://example.test/live"}"#).await;
        let client = HttpElevenLabsClient::new().expect("client");
        let minted = client
            .signed_url(&live_config(api_base))
            .await
            .expect("mints");
        assert_eq!(minted.signed_url, "wss://example.test/live");
        assert_eq!(minted.agent_id, "agent_live");

        let head = server.await.expect("server");
        let request_line = head.lines().next().expect("request line");
        assert!(
            request_line.contains("/v1/convai/conversation/get-signed-url?agent_id=agent_live"),
            "unexpected request line: {request_line}"
        );
        assert!(
            !request_line.contains("xi-live-key-value"),
            "the account key reached the URL, where every proxy logs it: {request_line}"
        );
        assert!(
            head.to_lowercase()
                .contains("xi-api-key: xi-live-key-value"),
            "the key must travel in the xi-api-key header: {head}"
        );
    }

    #[tokio::test]
    async fn the_live_client_redacts_a_rejection_that_quotes_the_key_back() {
        let (api_base, server) = one_shot_server(
            "401 Unauthorized",
            r#"{"detail":{"message":"key xi-live-key-value is invalid"}}"#,
        )
        .await;
        let client = HttpElevenLabsClient::new().expect("client");
        let error = client
            .signed_url(&live_config(api_base))
            .await
            .expect_err("a 401 is not a success");
        let _ = server.await;
        let rendered = error.to_string();
        assert!(rendered.contains("401"), "{rendered}");
        assert!(
            !rendered.contains("xi-live-key-value"),
            "the key leaked out of a vendor error body: {rendered}"
        );
        assert!(rendered.contains(super::super::REDACTED), "{rendered}");
    }

    #[test]
    fn a_signed_url_is_passed_through_unchanged() {
        let value = serde_json::json!({
            "signed_url": "wss://api.elevenlabs.io/v1/convai/conversation?agent_id=agent_1&token=abc"
        });
        let minted = parse_signed_url(&value, "agent_1").expect("parses");
        assert_eq!(
            minted.signed_url,
            "wss://api.elevenlabs.io/v1/convai/conversation?agent_id=agent_1&token=abc"
        );
        assert_eq!(minted.agent_id, "agent_1");
        assert_eq!(
            minted.valid_for_seconds,
            super::super::DOCUMENTED_VALIDITY_SECONDS
        );
    }

    #[test]
    fn an_answer_without_a_signed_url_fails_loudly() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({ "signed_url": null }),
            serde_json::json!({ "signed_url": 7 }),
            serde_json::json!({ "signed_url": "   " }),
        ] {
            assert!(
                matches!(
                    parse_signed_url(&value, "agent_1"),
                    Err(SignedUrlError::Shape(_))
                ),
                "an unusable answer must not be handed to a browser: {value}"
            );
        }
    }
}
