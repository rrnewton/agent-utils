//! Provider-independent read-aloud capabilities, audio, and errors.
//!
//! A provider owns its settings. HTTP handlers ask for speech or report its playback mode without
//! knowing which credentials, network service, or device capability it uses.

use async_trait::async_trait;

/// How the web app plays messages with the selected provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Playback {
    /// The browser speaks text using the device's speech engine.
    Browser,
    /// The server returns encoded audio.
    Audio,
}

/// Public capabilities of the selected read-aloud provider; never contains credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Description {
    /// Stable provider identifier.
    pub backend: &'static str,
    /// Human-readable provider name.
    pub label: &'static str,
    /// Which playback interface the browser should use.
    pub playback: Playback,
    /// Whether browser speech must select a voice advertised as running locally.
    /// This is the browser's `localService` flag; the device engine controls actual networking.
    pub local_only: bool,
}

/// Why a message could not be read aloud.
#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    /// A required provider setting is absent; no request was attempted.
    #[error("{detail}")]
    NotConfigured {
        /// Provider's machine-readable failure code.
        code: &'static str,
        /// Redacted explanation including the setting to change.
        detail: String,
    },
    /// The provider rejected a request or could not be reached.
    #[error("{detail}")]
    Backend {
        /// Provider's machine-readable failure code.
        code: &'static str,
        /// Redacted explanation from the provider adapter.
        detail: String,
    },
    /// There was no speakable text, so no synthesis was requested.
    #[error("this message has no text to read aloud")]
    Empty,
    /// This provider speaks on the device and cannot generate audio on the server.
    #[error("read-aloud uses this device's voice; open the message view and tap Read")]
    BrowserPlaybackRequired,
}

impl SpeechError {
    /// Stable machine-readable code for the API layer.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured { code, .. } | Self::Backend { code, .. } => code,
            Self::Empty => "nothing_to_read",
            Self::BrowserPlaybackRequired => "browser_speech_required",
        }
    }
}

/// Encoded audio for one message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Speech {
    /// Encoded audio, passed through without transcoding.
    pub audio: Vec<u8>,
    /// MIME type of the encoded audio.
    pub content_type: String,
}

/// Audio arriving as it is generated.
///
/// The error parameter lets a provider's transport retain its detailed errors until its adapter
/// converts them into the shared [`SpeechError`] contract.
pub struct SpeechStream<E = SpeechError> {
    /// MIME type of the encoded audio.
    pub content_type: String,
    /// Successive chunks of encoded audio, or a failure during generation.
    pub chunks: futures_util::stream::BoxStream<'static, Result<bytes::Bytes, E>>,
}

impl<E> std::fmt::Debug for SpeechStream<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeechStream")
            .field("content_type", &self.content_type)
            .finish_non_exhaustive()
    }
}

/// Turns message text into speech using settings owned by the provider.
#[async_trait]
pub trait SpeechProvider: Send + Sync {
    /// Identify the provider and the playback interface clients should use.
    fn describe(&self) -> Description;

    /// Prepare reusable provider resources before a reader taps a message.
    ///
    /// The default does nothing; providers can resolve a voice or open a connection without
    /// making shared handlers know which preparation their service needs.
    ///
    /// # Errors
    /// Returns the same configuration or provider failures as [`SpeechProvider::speak`].
    async fn warm_up(&self) -> Result<(), SpeechError> {
        Ok(())
    }

    /// Generate encoded audio for one message at the requested pace.
    ///
    /// # Errors
    /// Returns [`SpeechError`] for empty text, unavailable settings, provider failures, or a
    /// provider whose speech runs in the browser instead of on the server.
    async fn speak(&self, text: &str, speed: Option<f64>) -> Result<Speech, SpeechError>;

    /// Stream encoded audio, or send a single chunk when a provider cannot stream.
    ///
    /// # Errors
    /// The same failures as [`SpeechProvider::speak`]. Failures after streaming begins arrive as
    /// errors in the stream.
    async fn speak_stream(
        &self,
        text: &str,
        speed: Option<f64>,
    ) -> Result<SpeechStream, SpeechError> {
        let spoken = self.speak(text, speed).await?;
        Ok(SpeechStream {
            content_type: spoken.content_type,
            chunks: Box::pin(futures_util::stream::once(async move {
                Ok(bytes::Bytes::from(spoken.audio))
            })),
        })
    }
}

/// Device speech, performed in the browser without a server synthesis request.
#[derive(Debug, Default)]
pub struct BrowserSpeech;

#[async_trait]
impl SpeechProvider for BrowserSpeech {
    fn describe(&self) -> Description {
        Description {
            backend: "browser",
            label: "Device voice",
            playback: Playback::Browser,
            local_only: true,
        }
    }

    async fn speak(&self, _text: &str, _speed: Option<f64>) -> Result<Speech, SpeechError> {
        Err(SpeechError::BrowserPlaybackRequired)
    }
}

/// Lowest supported read-aloud pace, relative to a voice's normal speed.
pub const MIN_SPEECH_SPEED: f64 = 0.5;
/// Highest supported read-aloud pace, relative to a voice's normal speed.
pub const MAX_SPEECH_SPEED: f64 = 2.0;

/// Clamp a requested pace to supported bounds, dropping non-finite values.
#[must_use]
pub fn clamp_speed(speed: Option<f64>) -> Option<f64> {
    let value = speed?;
    value
        .is_finite()
        .then(|| value.clamp(MIN_SPEECH_SPEED, MAX_SPEECH_SPEED))
}
