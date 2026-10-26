//! The real local-sidecar client: plain HTTP over loopback.

use std::time::Duration;

use async_trait::async_trait;

use super::{LocalError, LocalTtsClient, Voice};

/// A request, described independently of any HTTP client.
#[derive(Debug, PartialEq, Eq)]
pub struct PreparedRequest {
    /// HTTP method.
    pub method: &'static str,
    /// Fully qualified URL on the sidecar's loopback endpoint.
    pub url: String,
}

/// Build the request that synthesizes one piece of text.
#[must_use]
pub fn synthesize_request(endpoint: &str) -> PreparedRequest {
    PreparedRequest {
        method: "POST",
        url: format!("{}/synthesize", endpoint.trim_end_matches('/')),
    }
}

/// Build the request that lists the sidecar's voices.
#[must_use]
pub fn voices_request(endpoint: &str) -> PreparedRequest {
    PreparedRequest {
        method: "GET",
        url: format!("{}/voices", endpoint.trim_end_matches('/')),
    }
}

/// The body that goes with [`synthesize_request`]. `speed` (a multiplier)
/// is sent only when the reader asked for a pace other than the voice's own.
#[must_use]
pub fn synthesize_body(text: &str, voice: &str, speed: Option<f64>) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    body.insert("q".to_owned(), serde_json::Value::String(text.to_owned()));
    body.insert(
        "voice".to_owned(),
        serde_json::Value::String(voice.to_owned()),
    );
    if let Some(speed) = speed.and_then(serde_json::Number::from_f64) {
        body.insert("speed".to_owned(), serde_json::Value::Number(speed));
    }
    serde_json::Value::Object(body)
}

/// Read the voices out of the answer to [`voices_request`], tolerantly:
/// entries naming their voice under `name` or `voice`, or bare strings.
#[must_use]
pub fn parse_voices(value: &serde_json::Value) -> Vec<Voice> {
    let entries = value
        .get("voices")
        .and_then(serde_json::Value::as_array)
        .or_else(|| value.as_array());
    let Some(entries) = entries else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
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
                .or_else(|| text("voice"))?;
            Some(Voice {
                name,
                locale: text("locale").unwrap_or_else(|| "en".to_owned()),
                gender: text("gender"),
                styles: Vec::new(),
            })
        })
        .collect()
}

/// How long one sidecar call may take before it is given up.
///
/// Local synthesis of one sentence is about a second on this class of
/// CPU (measured in `#561`); sixty still ends a wedged sidecar while the
/// reader waits, rather than at the page's patience. A cold first call
/// may load the model, which the sidecar does at startup instead — see
/// its `--warm` default.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// A live local-sidecar client.
#[derive(Debug)]
pub struct HttpLocalTtsClient {
    client: reqwest::Client,
    endpoint: String,
}

impl HttpLocalTtsClient {
    /// Build a client for `endpoint` (the sidecar's loopback base URL).
    ///
    /// # Errors
    ///
    /// Returns [`LocalError::Transport`] when the HTTP client cannot be built.
    pub fn new(endpoint: &str) -> Result<Self, LocalError> {
        let client = crate::egress::client_builder()
            .user_agent(concat!(
                "vibe-talk (https://github.com/rrnewton/agent-utils, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| LocalError::Transport(e.to_string()))?;
        Ok(Self {
            client,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
        })
    }
}

#[async_trait]
impl LocalTtsClient for HttpLocalTtsClient {
    async fn synthesize(
        &self,
        text: &str,
        voice: &str,
        speed: Option<f64>,
    ) -> Result<Vec<u8>, LocalError> {
        let request = synthesize_request(&self.endpoint);
        let response = self
            .client
            .post(&request.url)
            .json(&synthesize_body(text, voice, speed))
            .send()
            .await
            .map_err(|e| LocalError::Transport(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(LocalError::from_response(status.as_u16(), &body));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| LocalError::Transport(e.to_string()))
    }

    async fn voices(&self) -> Result<Vec<Voice>, LocalError> {
        let request = voices_request(&self.endpoint);
        let response = self
            .client
            .get(&request.url)
            .send()
            .await
            .map_err(|e| LocalError::Transport(e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| LocalError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(LocalError::from_response(status.as_u16(), &text));
        }
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| LocalError::Shape(e.to_string()))?;
        Ok(parse_voices(&value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_urls_match_the_sidecar_contract() {
        assert_eq!(
            synthesize_request("http://127.0.0.1:18771/").url,
            "http://127.0.0.1:18771/synthesize"
        );
        assert_eq!(
            voices_request("http://127.0.0.1:18771").url,
            "http://127.0.0.1:18771/voices"
        );
    }

    #[test]
    fn voices_parse_from_the_sidecar_shape() {
        let value = serde_json::json!({"voices": [{"name": "af_heart", "locale": "en"}, "am_michael", {"x": 1}]});
        let voices = parse_voices(&value);
        assert_eq!(voices.len(), 2);
        assert_eq!(voices[0].name, "af_heart");
        assert_eq!(voices[1].locale, "en");
    }
}
