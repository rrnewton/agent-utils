//! Read-aloud speech synthesis on this machine's own CPU.
//!
//! `#561 local-tts-provider`. The owner asked for message reading that
//! does not depend on a cloud service or the private voice stack: an
//! open-source TTS model (Kokoro-82M, Apache-2.0; or Piper, MIT) running
//! in a sidecar process beside this server — see
//! `scripts/local_tts_sidecar.py`. Synthesis makes no network call beyond
//! loopback to that sidecar, and no call at all off this machine.
//!
//! The sidecar contract is deliberately the shape [`crate::wit`] already
//! speaks, so the two providers share chunking, merging and silence
//! checks and a fallback chain can order them without translation:
//!
//! * `GET {endpoint}/voices` → `{"voices": [{"name": ..., "locale": ...}]}`
//! * `POST {endpoint}/synthesize` with JSON `{"q": text, "voice": name,
//!   "speed": multiplier}` → `audio/wav` bytes.
//!
//! The honesty rules are the same as every other backend's: a sidecar
//! that is down, refuses, stalls or answers silence stops the read with
//! its own clear error. Nothing is ever played as if it had spoken.

pub mod fake;
pub mod http;
pub mod speech;

use async_trait::async_trait;

/// One voice the local model offers, as the sidecar reports it.
pub type Voice = crate::speech::VoiceInfo;

/// Why one local synthesis call failed, before the adapter words it.
#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    /// The sidecar answered with a non-success status.
    #[error("the local speech sidecar returned HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body, truncated.
        body: String,
    },
    /// The sidecar never answered, or took too long.
    #[error("the local speech sidecar could not be reached: {0}")]
    Transport(String),
    /// The answer was not usable audio or not the expected shape.
    #[error("the local speech sidecar answered with something this server cannot use: {0}")]
    Shape(String),
}

impl LocalError {
    /// Build a status error from a sidecar response body, truncated on a
    /// character boundary (the body is sidecar text; `String::truncate`
    /// panics mid-character).
    #[must_use]
    pub fn from_response(status: u16, body: &str) -> Self {
        let mut body = body.to_owned();
        let mut end = body.len().min(500);
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        body.truncate(end);
        Self::Status { status, body }
    }

    /// Stable machine-readable code for the shared speech boundary.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Status { .. } => "local_tts_error",
            Self::Transport(_) => "local_tts_unreachable",
            Self::Shape(_) => "local_tts_bad_response",
        }
    }
}

/// The two sidecar calls read-aloud needs, behind one trait so tests can
/// stand in a fake that can refuse, stall and stay silent.
#[async_trait]
pub trait LocalTtsClient: Send + Sync {
    /// Synthesize ONE sentence-sized piece of text; returns WAV bytes.
    ///
    /// # Errors
    ///
    /// Returns [`LocalError`] for a sidecar refusal, a transport failure,
    /// or an unusable answer.
    async fn synthesize(
        &self,
        text: &str,
        voice: &str,
        speed: Option<f64>,
    ) -> Result<Vec<u8>, LocalError>;

    /// Every voice the sidecar offers, in its own order.
    ///
    /// # Errors
    ///
    /// As [`LocalTtsClient::synthesize`], minus the audio failures.
    async fn voices(&self) -> Result<Vec<Voice>, LocalError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_multibyte_error_body_truncates_without_panicking() {
        let error = LocalError::from_response(500, &"€".repeat(400));
        assert!(error.to_string().len() < 700, "{error}");
        assert_eq!(error.code(), "local_tts_error");
    }
}
