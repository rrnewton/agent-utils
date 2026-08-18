//! Shared server state, and the two policy decisions every handler goes through.

use std::sync::Arc;

use crate::agent_backend::AgentBackend;
use crate::config::Config;
use crate::discord::DiscordClient;
use crate::model::{ChannelId, ChannelInfo};
use crate::retrieval::Ranker;

/// Everything a request handler needs.
#[derive(Clone)]
pub struct AppState {
    /// Loaded configuration.
    pub config: Arc<Config>,
    /// Discord access.
    pub discord: Arc<dyn DiscordClient>,
    /// Strategy for semantic random access.
    pub ranker: Arc<dyn Ranker>,
    /// Slow-path backend (absent in v0).
    pub agent: Arc<dyn AgentBackend>,
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
}
