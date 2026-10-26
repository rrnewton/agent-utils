//! The real Wit.ai client.
//!
//! Same shape as [`crate::elevenlabs::http`]: which URL, which header and
//! how the answer parses are pure functions, unit-tested here; the async
//! wrapper around them is deliberately thin.

use std::time::Duration;

use async_trait::async_trait;

use super::{Voice, WitClient, WitError, API_VERSION, AUDIO_ACCEPT};
use crate::config::Secret;

/// A request, described independently of any HTTP client.
#[derive(Debug, PartialEq, Eq)]
pub struct PreparedRequest {
    /// HTTP method.
    pub method: &'static str,
    /// Fully qualified URL. Carries the API version only; never the token.
    pub url: String,
}

/// Build the request that synthesizes one piece of text.
///
/// Documented as `POST /synthesize?v=<version>` with the app token in an
/// `Authorization: Bearer` header. The token deliberately does NOT go in
/// the query string: a URL is the most-logged part of a request.
#[must_use]
pub fn synthesize_request(api_base: &str) -> PreparedRequest {
    PreparedRequest {
        method: "POST",
        url: format!(
            "{}/synthesize?v={API_VERSION}",
            api_base.trim_end_matches('/')
        ),
    }
}

/// Build the request that lists the app's voices: `GET /voices?v=<version>`.
#[must_use]
pub fn voices_request(api_base: &str) -> PreparedRequest {
    PreparedRequest {
        method: "GET",
        url: format!("{}/voices?v={API_VERSION}", api_base.trim_end_matches('/')),
    }
}

/// The body that goes with [`synthesize_request`].
///
/// `speed` is sent only when the reader asked for a pace other than the
/// voice's own; an absent field takes the vendor's default rather than a
/// value this server made up.
#[must_use]
pub fn synthesize_body(text: &str, voice: &str, speed_percent: Option<u32>) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert("q".to_owned(), serde_json::Value::String(text.to_owned()));
    body.insert(
        "voice".to_owned(),
        serde_json::Value::String(voice.to_owned()),
    );
    if let Some(speed) = speed_percent {
        body.insert("speed".to_owned(), serde_json::Value::Number(speed.into()));
    }
    serde_json::Value::Object(body)
}

/// Read the voices out of the answer to [`voices_request`].
///
/// The vendor answers a map of locale to a list of entries. Entries are
/// read tolerantly — an object naming its voice under `name` or `voice`,
/// or a bare string — because this parser's job is to fill a picker, and
/// refusing the whole list over one renamed field would leave the picker
/// empty beside a working synthesizer. Entries with no name at all are
/// skipped: a picker row that cannot be named cannot be chosen.
#[must_use]
pub fn parse_voices(value: &serde_json::Value) -> Vec<Voice> {
    let mut voices = Vec::new();
    let Some(locales) = value.as_object() else {
        return voices;
    };
    for (locale, entries) in locales {
        let Some(entries) = entries.as_array() else {
            continue;
        };
        for entry in entries {
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|found| !found.is_empty())
                    .map(str::to_owned)
            };
            let name = entry
                .as_str()
                .map(str::to_owned)
                .or_else(|| text("name"))
                .or_else(|| text("voice"));
            let Some(name) = name else { continue };
            voices.push(Voice {
                name,
                locale: locale.clone(),
                gender: text("gender"),
                styles: entry
                    .get("styles")
                    .and_then(serde_json::Value::as_array)
                    .map(|styles| {
                        styles
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
    }
    voices
}

/// How many times one synthesize call is attempted in total.
///
/// The first try plus three retries against a rate limit that is still
/// clearing. Bounded because a read is a person waiting: past this the
/// honest answer is the rate-limit message, not a longer silence.
pub const MAX_ATTEMPTS: u32 = 4;

/// The wait before the first retry of a rate-limited call, doubled per
/// retry when the vendor names no wait of its own.
pub const FIRST_BACKOFF: Duration = Duration::from_millis(500);

/// How long one Wit.ai call may take before it is given up as unreachable.
///
/// Synthesis of one sentence is seconds of work at the vendor; thirty is
/// generous headroom that still ends a wedged call while the reader is
/// waiting, rather than at the page's own patience.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A live Wit.ai client.
#[derive(Debug)]
pub struct HttpWitClient {
    client: reqwest::Client,
    api_base: String,
}

impl HttpWitClient {
    /// Build a client for `api_base` (no trailing slash needed).
    ///
    /// # Errors
    ///
    /// Returns [`WitError::Transport`] when the HTTP client cannot be built.
    pub fn new(api_base: &str) -> Result<Self, WitError> {
        let client = crate::egress::client_builder()
            .user_agent(concat!(
                "vibe-talk (https://github.com/rrnewton/agent-utils, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| WitError::Transport(e.to_string()))?;
        Ok(Self {
            client,
            api_base: api_base.trim_end_matches('/').to_owned(),
        })
    }

    /// One POST of `body`, backing off and retrying while Wit.ai answers
    /// 429. Returns the successful response, or the last failure.
    async fn post_synthesize(
        &self,
        token: &Secret,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, WitError> {
        let request = synthesize_request(&self.api_base);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = self
                .client
                .post(&request.url)
                .header(
                    reqwest::header::AUTHORIZATION,
                    format!("Bearer {}", token.expose()),
                )
                .header(reqwest::header::ACCEPT, AUDIO_ACCEPT)
                .json(body)
                .send()
                .await
                .map_err(|e| WitError::Transport(super::redact(&e.to_string(), token)))?;
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let text = response.text().await.unwrap_or_default();
            let error = WitError::from_response(status.as_u16(), &text, token, attempt);
            let WitError::RateLimited { .. } = error else {
                return Err(error);
            };
            if attempt >= MAX_ATTEMPTS {
                return Err(error);
            }
            let wait = retry_after
                .unwrap_or_else(|| FIRST_BACKOFF.saturating_mul(1 << (attempt - 1)))
                .min(Duration::from_secs(30));
            tokio::time::sleep(wait).await;
        }
    }
}

#[async_trait]
impl WitClient for HttpWitClient {
    async fn synthesize(
        &self,
        token: &Secret,
        text: &str,
        voice: &str,
        speed_percent: Option<u32>,
    ) -> Result<Vec<u8>, WitError> {
        let body = synthesize_body(text, voice, speed_percent);
        let response = self.post_synthesize(token, &body).await?;
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| WitError::Transport(super::redact(&e.to_string(), token)))
    }

    async fn voices(&self, token: &Secret) -> Result<Vec<Voice>, WitError> {
        let request = voices_request(&self.api_base);
        // The same bounded 429 backoff as synthesis: the Settings picker
        // and diagnostics call this, and a transient limit there is not a
        // broken token.
        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = self
                .client
                .get(&request.url)
                .header(
                    reqwest::header::AUTHORIZATION,
                    format!("Bearer {}", token.expose()),
                )
                .send()
                .await
                .map_err(|e| WitError::Transport(super::redact(&e.to_string(), token)))?;
            let status = response.status();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let text = response
                .text()
                .await
                .map_err(|e| WitError::Transport(super::redact(&e.to_string(), token)))?;
            if status.is_success() {
                let value: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|e| WitError::Shape(super::redact(&e.to_string(), token)))?;
                return Ok(parse_voices(&value));
            }
            let error = WitError::from_response(status.as_u16(), &text, token, attempt);
            let WitError::RateLimited { .. } = error else {
                return Err(error);
            };
            if attempt >= MAX_ATTEMPTS {
                return Err(error);
            }
            let wait = retry_after
                .unwrap_or_else(|| FIRST_BACKOFF.saturating_mul(1 << (attempt - 1)))
                .min(Duration::from_secs(30));
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_urls_match_the_documented_endpoints() {
        assert_eq!(
            synthesize_request("https://api.wit.ai"),
            PreparedRequest {
                method: "POST",
                url: format!("https://api.wit.ai/synthesize?v={API_VERSION}"),
            }
        );
        assert_eq!(
            voices_request("https://api.wit.ai/").url,
            format!("https://api.wit.ai/voices?v={API_VERSION}")
        );
    }

    #[test]
    fn the_body_names_text_voice_and_an_asked_for_speed_only() {
        assert_eq!(
            synthesize_body("hello there", "wit$Remi", Some(150)),
            serde_json::json!({"q": "hello there", "voice": "wit$Remi", "speed": 150})
        );
        assert_eq!(
            synthesize_body("hello there", "wit$Remi", None),
            serde_json::json!({"q": "hello there", "voice": "wit$Remi"})
        );
    }

    #[test]
    fn voices_parse_from_the_documented_shape_and_tolerate_variants() {
        let value = serde_json::json!({
            "en": [
                {"name": "wit$Remi", "gender": "male", "styles": ["default"]},
                {"voice": "wit$Clara"},
                "wit$Bare",
                {"gender": "female"}
            ],
            "fr": [{"name": "wit$Lea"}],
            "broken": 42
        });
        let voices = parse_voices(&value);
        assert_eq!(voices.len(), 4);
        assert_eq!(voices[0].name, "wit$Remi");
        assert_eq!(voices[0].display_name(), "Remi");
        assert_eq!(voices[0].locale, "en");
        assert_eq!(voices[3].locale, "fr");
        assert!(parse_voices(&serde_json::json!([])).is_empty());
    }
}
