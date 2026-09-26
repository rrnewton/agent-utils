//! Shared server state, and the two policy decisions every handler goes through.

use std::sync::Arc;

use crate::agent_backend::AgentBackend;
use crate::chat::ChatClient;
use crate::config::Config;
use crate::conversation::ConversationalVoiceProvider;
use crate::elevenlabs::SignedUrlProvider;
use crate::live::LiveHub;
use crate::model::{ChannelId, ChannelInfo};
use crate::retrieval::Ranker;
use crate::speech::SpeechProvider;
use crate::store::{AddedChannel, StateStore};
use crate::summarize::Summarizer;

/// Everything a request handler needs.
#[derive(Clone)]
pub struct AppState {
    /// Loaded configuration.
    pub config: Arc<Config>,
    /// Access to the configured chat provider.
    pub chat: Arc<dyn ChatClient>,
    /// Strategy for semantic random access.
    pub ranker: Arc<dyn Ranker>,
    /// Mints short-lived signed conversation URLs for the configured ElevenLabs agent.
    pub elevenlabs: Arc<dyn SignedUrlProvider>,
    /// Opens a browser-ready conversational voice session through the selected provider.
    pub conversation: Arc<dyn ConversationalVoiceProvider>,
    /// Reads messages using the configured speech backend and advertises its playback interface.
    ///
    /// Separate from minting a conversation URL: device speech needs no account, and a server
    /// audio provider can read aloud without having a conversational agent.
    pub speech: Arc<dyn SpeechProvider>,
    /// Slow-path backend (absent in v0).
    pub agent: Arc<dyn AgentBackend>,
    /// Turns one long channel message into one short line.
    ///
    /// The shipped implementation asks the deployment's ElevenLabs conversational agent, so on a
    /// server with no credentials every call fails by name and the page draws those rows as
    /// failed. Behind a trait for the same reason the store is: a second backend must be
    /// substitutable without a call site changing, and the tests need one that counts.
    /// [`crate::summarize::Summarizer::describe`] is reported to the caller so a page can never
    /// imply a summary it did not get.
    pub summarizer: Arc<dyn Summarizer>,
    /// The policy every cached summary is filed under. Computed once at startup from
    /// [`crate::summarize::policy_version`], because recomputing it per request is how the two
    /// halves of a cache key drift apart.
    pub summary_version: Arc<str>,
    /// The one place this server keeps anything between restarts.
    ///
    /// Reached only through the trait, never as a concrete database, so the backend can be
    /// replaced without touching a handler. When nothing is configured this is
    /// [`crate::store::disabled::DisabledStore`], which refuses every call and names the setting
    /// to add — it is deliberately not a silent in-memory substitute. See [`crate::store`].
    pub store: Arc<dyn StateStore>,
    /// The live fan-out: where inbound Discord messages are published, and where the SSE route
    /// subscribes.
    ///
    /// Always present, even when ingestion is off. An absent hub would make the stream route
    /// conditional on configuration, and a route that 404s for a reason unrelated to the channel
    /// allowlist is the kind of ambiguity this server spends effort avoiding. With ingestion off
    /// nobody publishes, so a subscriber simply waits — and the startup banner says which it is.
    pub live: Arc<LiveHub>,
    /// Messages resolved for server audio BEFORE the reader taps one.
    ///
    /// With server audio, turning read-aloud on prepares everything on screen and a tap then plays
    /// a ticket. Device speech reads the text already in the browser and uses no tickets.
    /// See [`crate::speech_tickets`] for why server audio uses this authority boundary.
    pub speech_tickets: Arc<crate::speech_tickets::SpeechTickets>,
    /// The letters standing in for the long ids and hashes in channel text.
    ///
    /// Here rather than per request because the point of a letter is that it means the same account
    /// in the next message as it did in this one, and a table rebuilt per request cannot promise
    /// that. Here rather than per reader because this server has ONE channel allowlist and every
    /// reader reads through it, so a letter never names a value some reader could not already see
    /// on their own screen. [`crate::speakable::Names`] states the rule that makes that sound.
    ///
    /// It does not survive a restart, and nothing about it needs to: a letter is only ever
    /// interpreted from inside a conversation that is also gone.
    pub spoken_names: Arc<crate::speakable::SharedNames>,
    /// Channels the owner added from inside the app, joined onto the configured allowlist.
    ///
    /// Held in memory as well as in the store because the allowlist is consulted on every
    /// channel-scoped request, including from [`AppState::channel`], which is synchronous by
    /// design. Loaded once at startup and kept in step by the add and remove routes; the store is
    /// what makes it survive a restart, not what is read on the hot path.
    pub added_channels: Arc<std::sync::RwLock<Vec<AddedChannel>>>,
    /// Serializes managed register/probe/persist and unregister/remove transactions.
    ///
    /// The provider and local store cannot share a transaction. Holding this lock across both
    /// sides ensures concurrent requests cannot observe the same channel as absent and then undo
    /// one another's upstream registration during compensation.
    pub channel_registration_lock: Arc<tokio::sync::Mutex<()>>,
    /// The one post a voice agent has proposed and nobody has confirmed yet. `#34
    /// voice-chat-write-confirm`: over MCP, `post_reply` records a proposal here and posts nothing;
    /// only the commit route, which no tool reaches, can spend it. See [`crate::post_gate`].
    pub post_gate: Arc<crate::post_gate::PostGate>,
    /// Caps the lines `POST /api/v1/voice-health` writes, across every caller. Per process rather
    /// than per call because the server has no call id to count by, by design.
    pub voice_health_budget: Arc<crate::voice_health::LogBudget>,
}

fn added_channel_info(row: &AddedChannel, active_provider: Option<&str>) -> ChannelInfo {
    let provider_matches = row
        .registration_provider
        .as_deref()
        .is_none_or(|provider| Some(provider) == active_provider);
    ChannelInfo {
        id: row.channel.clone(),
        label: row.label.clone(),
        writable: row.writable && provider_matches,
        alias: None,
        added: true,
    }
}

impl AppState {
    /// Restore channels previously added from inside the app into the live allowlist.
    ///
    /// The store is read once at startup. Keeping this operation on [`AppState`] makes the same
    /// restored set visible to request handlers, diagnostics, and the live poller instead of
    /// requiring each consumer to rebuild its own copy.
    pub async fn restore_added_channels(&self) -> Result<usize, crate::store::StoreError> {
        let restored = self.store.added_channels().await?;
        let count = restored.len();
        let mut slot = self.added_channels.write().map_err(|_| {
            crate::store::StoreError::Backend(
                "the in-memory added-channel allowlist lock is poisoned".to_owned(),
            )
        })?;
        *slot = restored;
        Ok(count)
    }

    /// Canonical namespace for provider-managed registrations in the active configuration.
    #[must_use]
    pub fn registration_provider(&self) -> Option<&str> {
        self.chat
            .supports_channel_registration()
            .then(|| self.config.discord.api_base.trim_end_matches('/'))
    }

    /// Look up a configured channel.
    ///
    /// A channel that is not configured does not exist as far as this server is concerned. This is
    /// the allowlist: the bot may be in many channels, but only these are reachable through the
    /// API, so a guessed snowflake cannot turn the bridge into a general-purpose Discord reader.
    /// Configured channels are searched first, then the ones added from inside the app. A
    /// configured entry WINS: the file is the operator's standing statement, and an added row that
    /// shadowed it could silently change whether the bridge may post somewhere.
    #[must_use]
    pub fn channel(&self, id: &str) -> Option<ChannelInfo> {
        if let Some(found) = self.config.channels.iter().find(|c| c.id.as_str() == id) {
            return Some(found.clone());
        }
        let active_provider = self.registration_provider();
        self.added_channels
            .read()
            .ok()?
            .iter()
            .find(|c| c.channel.as_str() == id)
            .map(|row| added_channel_info(row, active_provider))
    }

    /// Every channel this server will answer for: configured first, then added, in that order.
    #[must_use]
    pub fn all_channels(&self) -> Vec<ChannelInfo> {
        let mut all = self.config.channels.clone();
        if let Ok(added) = self.added_channels.read() {
            let active_provider = self.registration_provider();
            for channel in added
                .iter()
                .map(|row| added_channel_info(row, active_provider))
            {
                if !all.iter().any(|c| c.id == channel.id) {
                    all.push(channel);
                }
            }
        }
        all
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
