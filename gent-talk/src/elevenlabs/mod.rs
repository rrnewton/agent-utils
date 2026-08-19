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
