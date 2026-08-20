//! Shared server state, and the two policy decisions every handler goes through.

use std::sync::Arc;

use crate::agent_backend::AgentBackend;
use crate::config::Config;
use crate::discord::DiscordClient;
use crate::elevenlabs::SignedUrlProvider;
use crate::model::{ChannelId, ChannelInfo};
use crate::retrieval::Ranker;
use crate::store::StateStore;

/// Everything a request handler needs.
#[derive(Clone)]
pub struct AppState {
    /// Loaded configuration.
    pub config: Arc<Config>,
    /// Discord access.
    pub discord: Arc<dyn DiscordClient>,
    /// Strategy for semantic random access.
    pub ranker: Arc<dyn Ranker>,
    /// Mints short-lived signed conversation URLs for the configured ElevenLabs agent.
    pub elevenlabs: Arc<dyn SignedUrlProvider>,
    /// Slow-path backend (absent in v0).
    pub agent: Arc<dyn AgentBackend>,
    /// The one place this server keeps anything between restarts.
    ///
    /// Reached only through the trait, never as a concrete database, so the backend can be
    /// replaced without touching a handler. When nothing is configured this is
    /// [`crate::store::disabled::DisabledStore`], which refuses every call and names the setting
    /// to add — it is deliberately not a silent in-memory substitute. See [`crate::store`].
    pub store: Arc<dyn StateStore>,
}

impl AppState {
    /// Look up a configured channel.
    ///
    /// A channel that is not configured does not exist as far as this server is concerned. This is
    /// the allowlist: the bot may be in many channels, but only these are reachable through the
    /// API, so a guessed snowflake cannot turn the bridge into a general-purpose Discord reader.
    #[must_use]
    pub fn channel(&self, id: &str) -> Option<&ChannelInfo> {
        self.config.channels.iter().find(|c| c.id.as_str() == id)
    }

    /// Resolve a caller-requested fetch size against the configured default and ceiling.
    #[must_use]
    pub fn effective_limit(&self, requested: Option<u16>) -> u16 {
        let discord = &self.config.discord;
        requested
            .unwrap_or(discord.default_fetch_limit)
            .clamp(1, discord.max_fetch_limit)
    }

    /// Resolve a caller-requested count ceiling against the configured one.
    ///
    /// The configured value is a ceiling, never a floor: a caller may ask to look at fewer
    /// messages than the operator allows, and may not ask to look at more. Counting costs one
    /// Discord request per hundred messages against a shared rate limit, so this is the knob that
    /// keeps "how many messages are in there?" from being an expensive question.
    #[must_use]
    pub fn effective_count_cap(&self, requested: Option<u32>) -> u32 {
        let configured = self.config.discord.max_count_scan;
        requested.map_or(configured, |r| r.clamp(1, configured))
    }

    /// Channel ids this server is configured for, in configuration order.
    #[must_use]
    pub fn channel_ids(&self) -> Vec<ChannelId> {
        self.config.channels.iter().map(|c| c.id.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::testing;

    #[test]
    fn an_unconfigured_channel_is_invisible() {
        let (state, _fake) = testing::state();
        assert!(state.channel("999").is_none());
        assert!(state.channel(testing::READ_CHANNEL).is_some());
    }

    #[test]
    fn the_fetch_limit_is_defaulted_and_capped() {
        let (state, _fake) = testing::state();
        let max = state.config.discord.max_fetch_limit;
        let default = state.config.discord.default_fetch_limit;
        assert_eq!(state.effective_limit(None), default);
        assert_eq!(state.effective_limit(Some(1)), 1);
        assert_eq!(state.effective_limit(Some(0)), 1);
        assert_eq!(
            state.effective_limit(Some(u16::MAX)),
            max,
            "a caller must not be able to request an unbounded fetch"
        );
    }

    #[test]
    fn the_count_ceiling_is_a_ceiling_and_not_a_floor() {
        let (state, _fake) = testing::state();
        let configured = state.config.discord.max_count_scan;
        assert_eq!(state.effective_count_cap(None), configured);
        assert_eq!(state.effective_count_cap(Some(10)), 10);
        assert_eq!(state.effective_count_cap(Some(0)), 1);
        assert_eq!(
            state.effective_count_cap(Some(u32::MAX)),
            configured,
            "a caller must not be able to make this server walk a channel's whole history"
        );
    }
}
