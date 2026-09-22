//! Provider-neutral conversational voice session discovery.
//!
//! The browser owns the realtime socket. This module only decides which wire protocol it should
//! speak and hands it the endpoint after the normal write-scope authorization check.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;

use crate::config::{ConversationBackend, ConversationConfig, ElevenLabsConfig};
use crate::elevenlabs::{SignedUrlError, SignedUrlProvider};

/// Stable provider details that can be shown before a session is opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VoiceDescription {
    /// Human-readable provider name.
    pub name: String,
    /// Provider console for the configured agent, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_url: Option<String>,
    /// Provider instance identifier, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
}

/// A browser-ready conversational voice session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VoiceSession {
    /// WebSocket endpoint for this conversation.
    pub websocket_url: String,
    /// Wire protocol spoken on the socket.
    pub protocol: &'static str,
    /// Human-readable provider label for connection details.
    pub provider: String,
    /// Endpoint validity when the provider issues expiring URLs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_for_seconds: Option<u32>,
    /// Microphone PCM sample rate expected by the provider.
    pub input_sample_rate: u32,
    /// Speaker PCM sample rate emitted by the provider.
    pub output_sample_rate: u32,
}

/// Why a voice session could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum VoiceSessionError {
    /// The selected provider is missing a required deployment setting.
    #[error("{0} is not configured; this server cannot open a voice conversation")]
    NotConfigured(&'static str),
    /// The selected provider failed while opening its session.
    #[error(transparent)]
    Provider(#[from] SignedUrlError),
}

impl VoiceSessionError {
    /// Stable API error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured(_) => "conversation_not_configured",
            Self::Provider(error) => error.code(),
        }
    }
}

/// Opens browser-ready sessions without exposing a provider-specific call site.
#[async_trait]
pub trait ConversationalVoiceProvider: Send + Sync {
    /// Describe the selected provider without opening a conversation.
    fn describe(&self) -> VoiceDescription;

    /// Open one conversational voice session.
    ///
    /// # Errors
    ///
    /// Returns [`VoiceSessionError`] when required configuration is absent or the provider fails.
    async fn open_session(&self) -> Result<VoiceSession, VoiceSessionError>;
}

/// Adapter from the existing signed-URL provider to the neutral session shape.
pub struct ElevenLabsConversationProvider {
    provider: Arc<dyn SignedUrlProvider>,
    config: ElevenLabsConfig,
}

impl ElevenLabsConversationProvider {
    /// Build an adapter over the existing ElevenLabs client.
    #[must_use]
    pub fn new(provider: Arc<dyn SignedUrlProvider>, config: ElevenLabsConfig) -> Self {
        Self { provider, config }
    }
}

#[async_trait]
impl ConversationalVoiceProvider for ElevenLabsConversationProvider {
    fn describe(&self) -> VoiceDescription {
        let instance_id = self
            .config
            .agent_id
            .as_deref()
            .map(str::trim)
            .filter(|id| {
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            })
            .map(str::to_owned);
        VoiceDescription {
            name: "ElevenLabs".to_owned(),
            settings_url: instance_id
                .as_ref()
                .map(|id| format!("https://elevenlabs.io/app/agents/{id}")),
            instance_id,
        }
    }

    async fn open_session(&self) -> Result<VoiceSession, VoiceSessionError> {
        let signed = self.provider.signed_url(&self.config).await?;
        let description = self.describe();
        Ok(VoiceSession {
            websocket_url: signed.signed_url,
            protocol: "elevenlabs",
            provider: description.name,
            valid_for_seconds: Some(signed.valid_for_seconds),
            input_sample_rate: 16_000,
            output_sample_rate: 16_000,
        })
    }
}

/// Provider for a deployment-managed WebSocket speaking the public `vibe-talk-v1` protocol.
pub struct WebSocketConversationProvider {
    url: Option<String>,
    label: String,
}

impl WebSocketConversationProvider {
    /// Build a provider from deployment configuration.
    #[must_use]
    pub fn new(config: &ConversationConfig) -> Self {
        Self {
            url: config.websocket_url.clone(),
            label: config.label.clone(),
        }
    }
}

#[async_trait]
impl ConversationalVoiceProvider for WebSocketConversationProvider {
    fn describe(&self) -> VoiceDescription {
        VoiceDescription {
            name: self.label.clone(),
            settings_url: None,
            instance_id: None,
        }
    }

    async fn open_session(&self) -> Result<VoiceSession, VoiceSessionError> {
        let url = self
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .ok_or(VoiceSessionError::NotConfigured(
                "conversation.websocket_url",
            ))?;
        Ok(VoiceSession {
            websocket_url: url.to_owned(),
            protocol: "vibe-talk-v1",
            provider: self.describe().name,
            valid_for_seconds: None,
            input_sample_rate: 24_000,
            output_sample_rate: 24_000,
        })
    }
}

/// Build the selected conversational provider once at startup.
#[must_use]
pub fn provider(
    config: &ConversationConfig,
    elevenlabs: Arc<dyn SignedUrlProvider>,
    elevenlabs_config: ElevenLabsConfig,
) -> Arc<dyn ConversationalVoiceProvider> {
    match config.backend {
        ConversationBackend::ElevenLabs => Arc::new(ElevenLabsConversationProvider::new(
            elevenlabs,
            elevenlabs_config,
        )),
        ConversationBackend::WebSocket => Arc::new(WebSocketConversationProvider::new(config)),
    }
}
