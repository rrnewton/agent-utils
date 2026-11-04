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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
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
    /// The owner's words for the agent, for the page to hand over as the socket opens. `#272
    /// voice-agent-instructions`.
    ///
    /// Filled in by `GET /api/v1/voice-session` on every session of a hosted agent the page
    /// configures, whatever the owner has saved — `not_sent` when none of his words can reach it —
    /// so the page always learns what the call was given; a provider never sets it. A
    /// deployment-managed bridge reads the same words from `GET /api/v1/voice-agent` instead, and
    /// its sessions carry none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_prompt: Option<crate::contract::SessionPrompt>,
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
            agent_prompt: None,
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
            agent_prompt: None,
        })
    }
}

/// Provider for a deployment-managed WebSocket speaking `vibe-talk-v1` whose server
/// side is a native speech-to-speech stack (Allentown S2S behind the Cosmos shim)
/// instead of a cascaded ASR/LLM/TTS one. The page-facing wire is identical to
/// [`WebSocketConversationProvider`]; the S2S session identity and checkpoint tier
/// are deployment configuration the bridge presents to Cosmos, carried here so
/// one config file describes the whole deployment.
pub struct S2sConversationProvider {
    url: Option<String>,
    label: String,
    /// Cosmos `client_id` the deployment presents (its own, registered) identity.
    pub client_id: Option<String>,
    /// ServiceRouter tier serving the S2S checkpoint.
    pub tier: Option<String>,
}

impl S2sConversationProvider {
    /// Build a provider from deployment configuration.
    #[must_use]
    pub fn new(config: &ConversationConfig) -> Self {
        Self {
            url: config.websocket_url.clone(),
            label: config.label.clone(),
            client_id: config.s2s_client_id.clone(),
            tier: config.s2s_tier.clone(),
        }
    }
}

#[async_trait]
impl ConversationalVoiceProvider for S2sConversationProvider {
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
            agent_prompt: None,
        })
    }
}

/// What to warn about when the browser's voice socket names a host other than this server's own
/// public one, or `None`. `#209 voice-socket-open`.
///
/// The page opens `conversation.websocket_url` itself, from a page served by this host. An
/// absolute URL naming ANOTHER host sends that socket to the other host's front door, where the
/// sign-in this host's door granted does not apply, and every call is refused before it opens. A
/// shared configuration did exactly that for two days. A path (`/ws?…`) is joined to the page's
/// own origin and cannot go wrong this way, so it is never warned about; nor is a URL this server
/// cannot compare, for want of a `server.public_base_url` or of a host in either.
#[must_use]
pub fn foreign_socket_host(
    conversation: &ConversationConfig,
    public_base_url: Option<&str>,
) -> Option<String> {
    if !matches!(
        conversation.backend,
        ConversationBackend::WebSocket | ConversationBackend::S2s
    ) {
        return None;
    }
    let socket = conversation.websocket_url.as_deref()?.trim();
    if socket.starts_with('/') && !socket.starts_with("//") {
        return None;
    }
    let host = |text: &str| {
        url::Url::parse(text)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    };
    let named = host(socket)?;
    let own = host(public_base_url?.trim())?;
    (named != own).then(|| {
        format!(
            "conversation.websocket_url names the host {named}, but server.public_base_url is on \
             {own}. The page opens that socket from {own}, and another host's front door may not \
             accept this page's sign-in, so a call can fail before it connects. Give it as a path \
             such as /ws?mode=... to use the page's own host."
        )
    })
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
        ConversationBackend::S2s => Arc::new(S2sConversationProvider::new(config)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{TranscriptRole, VibeTalkV1ClientFrame, VibeTalkV1ServerFrame};
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;

    fn websocket(url: &str) -> ConversationConfig {
        ConversationConfig {
            backend: ConversationBackend::WebSocket,
            websocket_url: Some(url.to_owned()),
            label: "voice".to_owned(),
            ask_before_send: true,
            explicit_send_only: true,
            ..ConversationConfig::default()
        }
    }

    /// `#209 voice-socket-open`. The misconfiguration that kept every call on one host from
    /// connecting is named at startup; the shapes that cannot go wrong that way are not.
    #[test]
    fn a_socket_on_another_host_is_warned_about_and_nothing_else_is() {
        let own = Some("https://alpha.example.test:8443");
        let warning = foreign_socket_host(&websocket("wss://beta.example.test/ws?mode=x"), own)
            .expect("another host is warned about");
        assert!(warning.contains("beta.example.test"), "{warning}");
        assert!(warning.contains("alpha.example.test"), "{warning}");
        assert!(warning.contains("path"), "{warning}");

        for quiet in [
            "/ws?mode=x&tools=chat",
            "wss://alpha.example.test/ws",
            "wss://ALPHA.example.test:9443/ws",
            "not a url",
        ] {
            assert_eq!(foreign_socket_host(&websocket(quiet), own), None, "{quiet}");
        }
        assert_eq!(
            foreign_socket_host(&websocket("wss://beta.example.test/ws"), None),
            None,
            "nothing to compare with"
        );
        let hosted = ConversationConfig {
            backend: ConversationBackend::ElevenLabs,
            ..websocket("wss://beta.example.test/ws")
        };
        assert_eq!(
            foreign_socket_host(&hosted, own),
            None,
            "the hosted provider does not use this URL"
        );
    }

    fn s2s(url: &str) -> ConversationConfig {
        ConversationConfig {
            backend: ConversationBackend::S2s,
            websocket_url: Some(url.to_owned()),
            label: "voice (S2S)".to_owned(),
            s2s_client_id: Some("vibe-talk-devbig014".to_owned()),
            s2s_tier: Some("realtimeai.llm.test_v2".to_owned()),
            ask_before_send: true,
            explicit_send_only: true,
            ..ConversationConfig::default()
        }
    }

    /// A fake S2S bridge: speaks `vibe-talk-v1`, and for each closed audio
    /// segment answers with one transcript, one binary PCM frame and one
    /// `turn_complete` — the shape an Allentown S2S backend behind the Cosmos
    /// shim presents to the page. No real S2S tier is touched.
    async fn fake_s2s_bridge(listener: tokio::net::TcpListener) {
        while let Ok((stream, _)) = listener.accept().await {
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("fake bridge handshake");
            let started = serde_json::to_string(&VibeTalkV1ServerFrame::SessionStarted {
                greeting: Some(false),
                session_id: Some("fake-s2s".to_owned()),
                capabilities: vec!["replay".to_owned()],
            })
            .unwrap();
            socket.send(Message::Text(started.into())).await.unwrap();
            let mut heard_audio = 0usize;
            while let Some(Ok(frame)) = socket.next().await {
                match frame {
                    Message::Binary(bytes) => heard_audio += bytes.len(),
                    Message::Text(raw) => {
                        let client: VibeTalkV1ClientFrame =
                            serde_json::from_str(raw.as_ref()).expect("client frame");
                        match client {
                            VibeTalkV1ClientFrame::AudioEnd => {
                                assert!(heard_audio > 0, "a turn with no audio");
                                let text =
                                    serde_json::to_string(&VibeTalkV1ServerFrame::Transcript {
                                        role: TranscriptRole::Assistant,
                                        text: "fake s2s reply".to_owned(),
                                        turn: Some(1),
                                        is_final: Some(true),
                                    })
                                    .unwrap();
                                socket.send(Message::Text(text.into())).await.unwrap();
                                socket
                                    .send(Message::Binary(vec![0u8; 960].into()))
                                    .await
                                    .unwrap();
                                let done =
                                    serde_json::to_string(&VibeTalkV1ServerFrame::TurnComplete {
                                        turn: Some(1),
                                        interrupted: None,
                                    })
                                    .unwrap();
                                socket.send(Message::Text(done.into())).await.unwrap();
                            }
                            VibeTalkV1ClientFrame::Quit => break,
                            _ => {}
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }

    /// The S2S backend hands the page the same `vibe-talk-v1` session shape as
    /// the websocket backend, and one audio turn flows through a fake S2S
    /// bridge and back: PCM in, transcript + PCM + `turn_complete` out.
    #[tokio::test]
    async fn an_s2s_turn_flows_through_a_fake_bridge_and_back() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake bridge");
        let addr = listener.local_addr().unwrap();
        let bridge = tokio::spawn(fake_s2s_bridge(listener));
        let url = format!("ws://{addr}/ws");

        // The bridge hello the server asks at startup sees the fake bridge's
        // capabilities, exactly as with the websocket backend.
        let capabilities =
            crate::voice_bridge::hello(&url, &crate::bridge_dial::Credentials::default())
                .await
                .expect("fake bridge answers the hello");
        assert_eq!(capabilities, vec!["replay".to_owned()]);

        let config = s2s(&url);
        let provider = S2sConversationProvider::new(&config);
        assert_eq!(
            provider.client_id.as_deref(),
            Some("vibe-talk-devbig014"),
            "the deployment's own Cosmos client_id"
        );
        let session = provider.open_session().await.expect("s2s session");
        assert_eq!(session.protocol, "vibe-talk-v1");
        assert_eq!(session.websocket_url, url);
        assert_eq!(session.input_sample_rate, 24_000);
        assert_eq!(session.output_sample_rate, 24_000);

        let (mut socket, _) = tokio_tungstenite::connect_async(session.websocket_url.as_str())
            .await
            .expect("connect fake bridge");
        let first = socket.next().await.unwrap().unwrap();
        let started: VibeTalkV1ServerFrame =
            serde_json::from_str(first.into_text().unwrap().as_ref()).expect("session_started");
        assert_eq!(
            started,
            VibeTalkV1ServerFrame::SessionStarted {
                greeting: Some(false),
                session_id: Some("fake-s2s".to_owned()),
                capabilities: vec!["replay".to_owned()],
            }
        );
        socket
            .send(Message::Text(
                serde_json::to_string(&VibeTalkV1ClientFrame::AudioStart)
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
        socket
            .send(Message::Binary(vec![0u8; 1920].into()))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                serde_json::to_string(&VibeTalkV1ClientFrame::AudioEnd)
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();

        let mut transcript = None;
        let mut audio_bytes = 0usize;
        let mut completed = false;
        while let Some(Ok(frame)) = socket.next().await {
            match frame {
                Message::Binary(bytes) => audio_bytes += bytes.len(),
                Message::Text(raw) => {
                    let server: VibeTalkV1ServerFrame =
                        serde_json::from_str(raw.as_ref()).expect("server frame");
                    match server {
                        VibeTalkV1ServerFrame::Transcript { text, .. } => transcript = Some(text),
                        VibeTalkV1ServerFrame::TurnComplete { .. } => {
                            completed = true;
                            break;
                        }
                        other => panic!("unexpected server frame: {other:?}"),
                    }
                }
                _ => {}
            }
        }
        assert_eq!(transcript.as_deref(), Some("fake s2s reply"));
        assert_eq!(audio_bytes, 960, "the fake bridge's PCM reply");
        assert!(completed, "the turn ends with turn_complete");
        socket
            .send(Message::Text(
                serde_json::to_string(&VibeTalkV1ClientFrame::Quit)
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
        bridge.abort();
    }

    /// An S2S deployment with no socket URL is refused the same way the
    /// websocket backend refuses one, naming the missing setting.
    #[tokio::test]
    async fn an_s2s_backend_without_a_url_is_not_configured() {
        let config = ConversationConfig {
            backend: ConversationBackend::S2s,
            ..ConversationConfig::default()
        };
        let error = S2sConversationProvider::new(&config)
            .open_session()
            .await
            .expect_err("no URL, no session");
        assert_eq!(error.code(), "conversation_not_configured");
    }
}
