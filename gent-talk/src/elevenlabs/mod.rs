//! Minting a short-lived signed conversation URL from ElevenLabs.
//!
//! Turning on "Enable Authentication" on an ElevenLabs agent closes the public `talk-to` link: a
//! conversation can only be started from a **signed URL**, which is minted from ElevenLabs' API
//! with an account API key and is good for a documented fifteen minutes. That key is an account
//! credential — it can spend money and read the account's agents — so it lives here, on the
//! server, and the only thing that ever crosses the wire to a caller is the minted URL.
//!
//! Three rules hold in this module, and the tests exist to hold them:
//!
//! * **The key never appears in an output.** It travels in a request *header*, never in a URL or
//!   a body, and every error built from a vendor response goes through [`redact`] first — because
//!   an upstream is free to echo a credential back in its own error text, and that text ends up
//!   in a log and in an API response.
//! * **A missing key is a loud, specific failure.** There is no unsigned fallback: an agent with
//!   authentication enabled would simply refuse the connection later, in the browser, where the
//!   operator cannot see why.
//! * **The failure modes are distinguishable.** "You never configured a key", "ElevenLabs
//!   rejected the key", and "ElevenLabs is unreachable" are three different errors with three
//!   different HTTP statuses, because they have three different fixes.

pub mod fake;
pub mod http;
pub mod mock;

use async_trait::async_trait;

use crate::config::{ElevenLabsConfig, Secret};

/// Why a signed URL could not be minted.
#[derive(Debug, thiserror::Error)]
pub enum SignedUrlError {
    /// A required setting is absent, so no call was attempted.
    #[error("{0} is not configured; this server cannot mint a signed conversation URL")]
    NotConfigured(&'static str),
    /// The request never completed.
    #[error("elevenlabs request failed: {0}")]
    Transport(String),
    /// ElevenLabs answered with a non-success status.
    #[error("elevenlabs returned HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body, truncated and redacted.
        body: String,
    },
    /// The response did not have the shape this server expects.
    #[error("elevenlabs response could not be understood: {0}")]
    Shape(String),
}

impl SignedUrlError {
    /// Build a [`SignedUrlError::Status`] from a vendor response body.
    ///
    /// Truncates — an intermediary's error page is not short — and redacts, because the body is
    /// third-party text that may quote the credential we just sent it. Every construction of the
    /// `Status` variant goes through here so there is one place to check.
    #[must_use]
    pub fn from_response(status: u16, body: &str, api_key: &Secret) -> Self {
        let mut body = redact(body, api_key);
        body.truncate(500);
        Self::Status { status, body }
    }

    /// Stable machine-readable code for the API layer.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured(_) => "elevenlabs_not_configured",
            Self::Transport(_) | Self::Status { .. } | Self::Shape(_) => "elevenlabs_error",
        }
    }
}

/// Replace every occurrence of a secret with a marker.
///
/// A blank secret is not searched for: `str::replace` with an empty needle inserts the marker
/// between every character, which would be a spectacular way to mangle an error message.
#[must_use]
pub fn redact(text: &str, secret: &Secret) -> String {
    let needle = secret.expose();
    if needle.is_empty() {
        return text.to_owned();
    }
    text.replace(needle, REDACTED)
}

/// What [`redact`] leaves behind.
pub const REDACTED: &str = "<redacted>";

/// Fifteen minutes, per ElevenLabs' own documentation of the signed URL's lifetime. Recorded so
/// the web page can say something true about how long the operator has to press "start".
pub const DOCUMENTED_VALIDITY_SECONDS: u32 = 15 * 60;

/// The credentials a mint needs, checked together so the caller learns which one is absent.
///
/// # Errors
///
/// Returns [`SignedUrlError::NotConfigured`] naming the field the operator has to set. The key is
/// checked first because it is the one that is easy to forget: an agent id is visible in the
/// dashboard URL, whereas the key is shown once at creation.
pub fn credentials(config: &ElevenLabsConfig) -> Result<(&str, &Secret), SignedUrlError> {
    let api_key = config
        .api_key
        .as_ref()
        .filter(|key| !key.expose().trim().is_empty())
        .ok_or(SignedUrlError::NotConfigured("elevenlabs.api_key"))?;
    let agent_id = config
        .agent_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or(SignedUrlError::NotConfigured("elevenlabs.agent_id"))?;
    Ok((agent_id, api_key))
}

/// The account key, which read-aloud needs whichever voice it ends up using.
///
/// # Errors
///
/// [`SpeechError::NotConfigured`] naming the absent setting, before any call is attempted.
pub fn speech_key(config: &ElevenLabsConfig) -> Result<&Secret, SpeechError> {
    config
        .api_key
        .as_ref()
        .filter(|key| !key.expose().trim().is_empty())
        .ok_or(SpeechError::NotConfigured("elevenlabs.api_key"))
}

/// The voice the operator named explicitly, if they named one.
///
/// `None` is not an error here: an unset `voice_id` means "use the voice the AGENT already
/// speaks in", which is resolved against ElevenLabs. See [`SpeechProvider::speak`].
#[must_use]
pub fn configured_voice(config: &ElevenLabsConfig) -> Option<&str> {
    config
        .voice_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

/// The agent whose own voice read-aloud borrows when no `voice_id` is configured.
///
/// # Errors
///
/// [`SpeechError::NotConfigured`] when neither setting is present, naming BOTH -- either one fixes
/// it, and an error naming only one would send the operator to configure the wrong thing.
pub fn voice_source_agent(config: &ElevenLabsConfig) -> Result<&str, SpeechError> {
    config
        .agent_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or(SpeechError::NotConfigured(
            "elevenlabs.voice_id (or elevenlabs.agent_id, whose own voice would be used)",
        ))
}

/// Why a message could not be read aloud.
///
/// The same four distinguishable failures as [`SignedUrlError`], for the same reason: "you never
/// configured a voice", "ElevenLabs rejected the key", "ElevenLabs is unreachable" and "the answer
/// was not audio" have four different fixes and must not arrive as one generic error.
#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    /// A required setting is absent, so no call was attempted.
    #[error("{0} is not configured; this server cannot read a message aloud")]
    NotConfigured(&'static str),
    /// The request never completed.
    #[error("elevenlabs request failed: {0}")]
    Transport(String),
    /// ElevenLabs answered with a non-success status.
    #[error("elevenlabs returned HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body, truncated and redacted.
        body: String,
    },
    /// There was nothing worth sending, so nothing was sent.
    ///
    /// A separate variant rather than an empty success: spending a vendor call on a message with
    /// no speakable text bills the account for silence, and answering with zero bytes of audio
    /// would reach the reader as a player that does nothing and says nothing about why.
    #[error("this message has no text to read aloud")]
    Empty,
}

impl SpeechError {
    /// Build a [`SpeechError::Status`] from a vendor response body, truncated and redacted.
    ///
    /// Goes through the same [`redact`] the signed-URL path uses: an upstream is free to echo the
    /// credential back in its own error text, and that text ends up in a log and in an API answer.
    #[must_use]
    pub fn from_response(status: u16, body: &str, api_key: &Secret) -> Self {
        let mut body = redact(body, api_key);
        body.truncate(500);
        Self::Status { status, body }
    }
}

impl SpeechError {
    /// Stable machine-readable code for the API layer.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured(_) => "elevenlabs_not_configured",
            Self::Empty => "nothing_to_read",
            Self::Transport(_) | Self::Status { .. } => "elevenlabs_error",
        }
    }
}

/// Carry a signed-URL-shaped failure across to the speech side.
///
/// Read-aloud reuses the agent-configuration GET, so its failures arrive as [`SignedUrlError`].
/// They are the same three failures with the same three fixes; only the sentence differs.
#[must_use]
pub fn speech_from_signed(error: SignedUrlError) -> SpeechError {
    match error {
        SignedUrlError::NotConfigured(what) => SpeechError::NotConfigured(what),
        SignedUrlError::Transport(detail) => SpeechError::Transport(detail),
        SignedUrlError::Status { status, body } => SpeechError::Status { status, body },
        SignedUrlError::Shape(detail) => SpeechError::Transport(detail),
    }
}

/// How a voice should sound, as the configured agent already sounds.
///
/// Read-aloud borrows the agent's whole delivery, not merely its voice id. The owner set his agent
/// to speak faster than default and then heard read-aloud speak at default -- correctly, because
/// nothing was carrying the speed across. A voice at the wrong pace is a different voice.
///
/// Every field is optional and an absent one is simply not sent, so the vendor's own default
/// applies. This never invents a value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoiceStyle {
    /// The synthesis model the agent uses.
    pub model_id: Option<String>,
    /// 1.0 is the vendor's default; below is slower, above is faster.
    pub speed: Option<f64>,
    /// How consistent the delivery is between generations.
    pub stability: Option<f64>,
    /// How closely the output tracks the original voice.
    pub similarity_boost: Option<f64>,
}

/// Audio for one message, as the vendor returned it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Speech {
    /// The encoded audio. Handed to the caller verbatim; this server does not transcode.
    pub audio: Vec<u8>,
    /// What the audio IS, so the browser is told rather than left to sniff.
    pub content_type: String,
}

/// Turning a message's text into audio.
#[async_trait]
pub trait SpeechProvider: Send + Sync {
    /// Read `text` aloud in the configured voice.
    ///
    /// # Errors
    ///
    /// Returns [`SpeechError`] when a setting is absent, the text is empty, the request fails, or
    /// ElevenLabs refuses.
    ///
    /// `speed` overrides whatever the agent is configured with, for a reader who wants this
    /// particular pass faster or slower. `None` means "however the agent speaks".
    async fn speak(
        &self,
        config: &ElevenLabsConfig,
        text: &str,
        speed: Option<f64>,
    ) -> Result<Speech, SpeechError>;
}

/// The speeds this server will ask a vendor for.
///
/// Bounded because the reader drives it from a slider and the vendor charges per request: below a
/// half the words stop being words, and above double the audio is faster than it can be followed,
/// so both ends are a request nobody wanted to pay for.
pub const MIN_SPEECH_SPEED: f64 = 0.5;
/// See [`MIN_SPEECH_SPEED`].
pub const MAX_SPEECH_SPEED: f64 = 2.0;

/// Clamp a requested speed into the range this server will ask for, or drop it if it is not a
/// number at all.
#[must_use]
pub fn clamp_speed(speed: Option<f64>) -> Option<f64> {
    let value = speed?;
    if !value.is_finite() {
        return None;
    }
    Some(value.clamp(MIN_SPEECH_SPEED, MAX_SPEECH_SPEED))
}

/// A minted, short-lived conversation URL.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SignedUrl {
    /// The `wss://` URL the browser connects to. Short-lived, and itself a credential.
    pub signed_url: String,
    /// The agent it was minted for. Public: it identifies a widget, not an account.
    pub agent_id: String,
    /// How long ElevenLabs documents the URL as being valid for.
    pub valid_for_seconds: u32,
}

/// Something that can mint a signed conversation URL.
///
/// The configuration is passed in rather than captured at construction so that an implementation
/// cannot quietly answer from a credential the running server is not actually configured with.
#[async_trait]
pub trait SignedUrlProvider: Send + Sync {
    /// Mint a signed conversation URL for the configured agent.
    ///
    /// # Errors
    ///
    /// Returns [`SignedUrlError`] when a setting is absent, the request fails, ElevenLabs refuses,
    /// or the answer cannot be understood.
    async fn signed_url(&self, config: &ElevenLabsConfig) -> Result<SignedUrl, SignedUrlError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(agent: Option<&str>, key: Option<&str>) -> ElevenLabsConfig {
        ElevenLabsConfig {
            agent_id: agent.map(str::to_owned),
            api_key: key.map(Secret::new),
            api_base: crate::config::DEFAULT_ELEVENLABS_API_BASE.to_owned(),
            voice_id: None,
        }
    }

    #[test]
    fn a_missing_api_key_names_the_api_key() {
        let error = credentials(&config(Some("agent_1"), None)).expect_err("must refuse");
        assert!(
            matches!(&error, SignedUrlError::NotConfigured(field) if *field == "elevenlabs.api_key"),
            "unexpected error: {error}"
        );
        assert!(
            error.to_string().contains("elevenlabs.api_key"),
            "the operator must be told WHICH setting is missing: {error}"
        );
    }

    #[test]
    fn a_missing_agent_id_names_the_agent_id() {
        let error = credentials(&config(None, Some("xi-key"))).expect_err("must refuse");
        assert!(
            matches!(&error, SignedUrlError::NotConfigured(field) if *field == "elevenlabs.agent_id"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_blank_setting_counts_as_absent() {
        // A container that renders an unset variable as "" must not look configured.
        assert!(credentials(&config(Some("agent_1"), Some("   "))).is_err());
        assert!(credentials(&config(Some(""), Some("xi-key"))).is_err());
        let trimmable = config(Some(" agent_1 "), Some("xi-key"));
        let (agent, key) = credentials(&trimmable).expect("valid");
        assert_eq!(
            agent, "agent_1",
            "the agent id is trimmed before it is used"
        );
        assert_eq!(key.expose(), "xi-key");
    }

    #[test]
    fn a_vendor_body_that_echoes_the_key_is_redacted() {
        // Not hypothetical enough to skip: an upstream error message quoting the credential it
        // was given is a normal thing for an API to do, and this body reaches a log and a caller.
        let key = Secret::new("xi-secret-key-value");
        let error = SignedUrlError::from_response(
            401,
            r#"{"detail":{"status":"invalid_api_key","message":"xi-secret-key-value is invalid"}}"#,
            &key,
        );
        let rendered = error.to_string();
        assert!(
            !rendered.contains("xi-secret-key-value"),
            "leak: {rendered}"
        );
        assert!(rendered.contains(REDACTED), "{rendered}");
        assert!(
            rendered.contains("401"),
            "the status must survive: {rendered}"
        );
    }

    #[test]
    fn an_oversized_vendor_body_is_truncated() {
        let key = Secret::new("xi-secret-key-value");
        let error = SignedUrlError::from_response(502, &"x".repeat(10_000), &key);
        match error {
            SignedUrlError::Status { body, .. } => assert_eq!(body.len(), 500),
            other => panic!("expected a status error, got {other:?}"),
        }
    }

    #[test]
    fn redacting_against_a_blank_secret_does_not_shred_the_text() {
        assert_eq!(redact("hello", &Secret::new("")), "hello");
    }

    #[test]
    fn error_codes_separate_misconfiguration_from_vendor_failure() {
        assert_eq!(
            SignedUrlError::NotConfigured("elevenlabs.api_key").code(),
            "elevenlabs_not_configured"
        );
        assert_eq!(
            SignedUrlError::from_response(401, "no", &Secret::new("k")).code(),
            "elevenlabs_error"
        );
        assert_eq!(
            SignedUrlError::Transport("dns".to_owned()).code(),
            "elevenlabs_error"
        );
    }
}
