//! An in-memory ElevenLabs stand-in for tests and for local development.
//!
//! # It must be able to say no
//!
//! The same design note as [`crate::discord::fake`], for the same reason. A fake that answers
//! every mint with a plausible `wss://` URL cannot fail, so a test written against it would be
//! self-certifying: green here and worth nothing about the real client. This one therefore holds
//! the two things a real ElevenLabs account holds — **the API key it will accept**, and **the
//! agent ids it actually has** — and refuses anything else the way ElevenLabs refuses it: HTTP
//! 401 for a bad key, HTTP 404 for an agent that is not on the account.
//!
//! It also shares the real client's code for the two things that could otherwise drift: it builds
//! its request through [`super::http::signed_url_request`], and it goes through
//! [`super::credentials`] first, so a server with no key configured fails here exactly as it fails
//! in production — before any call is attempted, naming the setting.

use std::collections::BTreeSet;
use std::sync::Mutex;

use async_trait::async_trait;

use super::http::signed_url_request;
use super::{credentials, SignedUrl, SignedUrlError, SignedUrlProvider};
use crate::config::ElevenLabsConfig;

/// The API key [`FakeElevenLabs::new`] accepts. Anything else is a 401.
pub const VALID_API_KEY: &str = "xi-fake-api-key-not-a-real-one";
/// The agent id [`FakeElevenLabs::new`] knows about. Anything else is a 404.
pub const KNOWN_AGENT_ID: &str = "agent_fake0000000000000000000000";
/// The URL [`FakeElevenLabs::new`] mints.
pub const MINTED_URL: &str =
    "wss://api.elevenlabs.io/v1/convai/conversation?agent_id=agent_fake0000000000000000000000\
     &token=fake-conversation-token";

/// In-memory ElevenLabs.
#[derive(Debug)]
pub struct FakeElevenLabs {
    valid_api_key: String,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    known_agents: BTreeSet<String>,
    /// Every URL this fake was asked to fetch, in order. Lets a test prove a mint did NOT happen.
    requested: Vec<String>,
    minted_url: String,
    fail_with: Option<String>,
}

impl Default for FakeElevenLabs {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeElevenLabs {
    /// A fake account holding [`VALID_API_KEY`] and one agent, [`KNOWN_AGENT_ID`].
    #[must_use]
    pub fn new() -> Self {
        let mut known_agents = BTreeSet::new();
        known_agents.insert(KNOWN_AGENT_ID.to_owned());
        Self {
            valid_api_key: VALID_API_KEY.to_owned(),
            state: Mutex::new(State {
                known_agents,
                requested: Vec::new(),
                minted_url: MINTED_URL.to_owned(),
                fail_with: None,
            }),
        }
    }

    /// Declare that this account has `agent_id`.
    pub fn register_agent(&self, agent_id: &str) {
        self.lock().known_agents.insert(agent_id.to_owned());
    }

    /// Change what a successful mint returns, so pass-through can be asserted on a distinctive
    /// value rather than on a constant this module also produces by default.
    pub fn mint(&self, url: &str) {
        self.lock().minted_url = url.to_owned();
    }

    /// Make the next mint fail at the transport, so unreachability can be tested.
    pub fn fail_next(&self, detail: &str) {
        self.lock().fail_with = Some(detail.to_owned());
    }

    /// Every URL this fake was asked for, in order.
    #[must_use]
    pub fn requested(&self) -> Vec<String> {
        self.lock().requested.clone()
    }

    /// How many mints were attempted. "The route never called out" is otherwise
    /// indistinguishable from "it called out and the answer was discarded".
    #[must_use]
    pub fn attempts(&self) -> usize {
        self.lock().requested.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl SignedUrlProvider for FakeElevenLabs {
    async fn signed_url(&self, config: &ElevenLabsConfig) -> Result<SignedUrl, SignedUrlError> {
        // Same gate the real client goes through, first, so an unconfigured server cannot be made
        // to look configured by swapping in this fake.
        let (agent_id, api_key) = credentials(config)?;
        let request = signed_url_request(&config.api_base, agent_id);
        self.lock().requested.push(request.url);

        if let Some(detail) = self.lock().fail_with.take() {
            return Err(SignedUrlError::Transport(detail));
        }
        if api_key.expose() != self.valid_api_key {
            // ElevenLabs' own shape for a rejected key, and deliberately one that QUOTES the key
            // back: an upstream is free to do that, and the redaction in `from_response` is what
            // stops it reaching a log or a caller.
            return Err(SignedUrlError::from_response(
                401,
                &format!(
                    r#"{{"detail":{{"status":"invalid_api_key","message":"the key {} is not valid"}}}}"#,
                    api_key.expose()
                ),
                api_key,
            ));
        }
        if !self.lock().known_agents.contains(agent_id) {
            return Err(SignedUrlError::from_response(
                404,
                &format!(r#"{{"detail":{{"status":"agent_not_found","message":"{agent_id}"}}}}"#),
                api_key,
            ));
        }
        Ok(SignedUrl {
            signed_url: self.lock().minted_url.clone(),
            agent_id: agent_id.to_owned(),
            valid_for_seconds: super::DOCUMENTED_VALIDITY_SECONDS,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Secret, DEFAULT_ELEVENLABS_API_BASE};

    fn config(agent: Option<&str>, key: Option<&str>) -> ElevenLabsConfig {
        ElevenLabsConfig {
            agent_id: agent.map(str::to_owned),
            api_key: key.map(Secret::new),
            api_base: DEFAULT_ELEVENLABS_API_BASE.to_owned(),
        }
    }

    fn good() -> ElevenLabsConfig {
        config(Some(KNOWN_AGENT_ID), Some(VALID_API_KEY))
    }

    #[tokio::test]
    async fn a_correctly_configured_server_gets_a_url() {
        let fake = FakeElevenLabs::new();
        let minted = fake.signed_url(&good()).await.expect("mints");
        assert_eq!(minted.signed_url, MINTED_URL);
        assert_eq!(minted.agent_id, KNOWN_AGENT_ID);
        assert_eq!(fake.attempts(), 1);
        assert!(
            fake.requested()[0].contains(KNOWN_AGENT_ID),
            "the fake must exercise the real URL builder: {:?}",
            fake.requested()
        );
    }

    #[tokio::test]
    async fn a_wrong_key_is_refused_the_way_elevenlabs_refuses_it() {
        // The load-bearing property. If this fake minted a URL for any key, every test below it
        // would be certifying nothing.
        let fake = FakeElevenLabs::new();
        let error = fake
            .signed_url(&config(Some(KNOWN_AGENT_ID), Some("xi-wrong-key")))
            .await
            .expect_err("a key this account does not have must not mint a conversation");
        match error {
            SignedUrlError::Status { status, body } => {
                assert_eq!(status, 401);
                assert!(body.contains("invalid_api_key"), "{body}");
                assert!(!body.contains("xi-wrong-key"), "key echoed back: {body}");
            }
            other => panic!("expected a 401, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_missing_key_fails_before_any_call_is_attempted() {
        let fake = FakeElevenLabs::new();
        let error = fake
            .signed_url(&config(Some(KNOWN_AGENT_ID), None))
            .await
            .expect_err("an unconfigured server must not mint");
        assert!(
            matches!(&error, SignedUrlError::NotConfigured(f) if *f == "elevenlabs.api_key"),
            "unexpected error: {error}"
        );
        assert_eq!(
            fake.attempts(),
            0,
            "nothing should be requested when there is no key to request it with"
        );
    }

    #[tokio::test]
    async fn an_agent_this_account_does_not_have_is_a_404() {
        let fake = FakeElevenLabs::new();
        let error = fake
            .signed_url(&config(Some("agent_someone_elses"), Some(VALID_API_KEY)))
            .await
            .expect_err("an unknown agent must not mint");
        assert!(
            matches!(&error, SignedUrlError::Status { status, .. } if *status == 404),
            "unexpected error: {error}"
        );
        // Registering it is the counterpart of creating the agent on the account.
        fake.register_agent("agent_someone_elses");
        assert!(fake
            .signed_url(&config(Some("agent_someone_elses"), Some(VALID_API_KEY)))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn injected_transport_failures_surface_and_are_one_shot() {
        let fake = FakeElevenLabs::new();
        fake.fail_next("dns lookup failed");
        assert!(matches!(
            fake.signed_url(&good()).await,
            Err(SignedUrlError::Transport(_))
        ));
        assert!(fake.signed_url(&good()).await.is_ok());
    }
}
