//! Scaffolding shared by unit tests and the integration tests under `tests/`.
//!
//! This is test support, not product code: it builds a server whose Discord is
//! [`crate::discord::fake::FakeDiscord`]. The binary never calls it.
//!
//! Using it confines every HTTP client the process builds afterwards to loopback: see
//! [`crate::egress`].

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::agent_backend::NoAgentBackend;
use crate::config::Config;
use crate::discord::fake::FakeDiscord;
use crate::elevenlabs::fake::{FakeElevenLabs, KNOWN_AGENT_ID, VALID_API_KEY};
use crate::retrieval::LexicalRanker;
use crate::state::AppState;
use crate::store::fake::FakeStore;

/// Read-scope token used by tests.
pub const READ_TOKEN: &str = "test-read-token-000000000000";
/// Write-scope token used by tests.
pub const WRITE_TOKEN: &str = "test-write-token-00000000000";
/// A configured, read-only channel.
pub const READ_CHANNEL: &str = "1111111111";
/// A configured, writable channel.
pub const WRITE_CHANNEL: &str = "2222222222";

/// TOML for a server with one read-only and one writable channel.
#[must_use]
pub fn config_toml() -> String {
    // Its legacy `[discord]` section names no `api_base`, so a client built from it would name the
    // public Discord API.
    crate::egress::confine_to_loopback();
    format!(
        r#"
[server]
bind = "127.0.0.1:0"

[discord]
bot_token = "test-bot-token"
default_fetch_limit = 20
max_fetch_limit = 50

[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[[channels]]
id = "{READ_CHANNEL}"
label = "build noise"
writable = false

[[channels]]
id = "{WRITE_CHANNEL}"
label = "lead team"
writable = true

[elevenlabs]
agent_id = "{KNOWN_AGENT_ID}"
api_key = "{VALID_API_KEY}"
"#
    )
}

/// TOML for a server whose ElevenLabs half was never wired up.
///
/// Kept as a first-class fixture rather than a string a test edits inline, because "the operator
/// forgot the key" is a state this server has to handle explicitly, not an edge case.
#[must_use]
pub fn config_toml_without_elevenlabs() -> String {
    config_toml()
        .split("[elevenlabs]")
        .next()
        .expect("prefix")
        .to_owned()
}

/// A parsed test configuration.
///
/// # Panics
///
/// Panics if the embedded test configuration stops being valid, which is itself a test failure.
#[must_use]
pub fn config() -> Config {
    crate::egress::confine_to_loopback();
    Config::from_toml_and_env(&config_toml(), &BTreeMap::new()).expect("test config is valid")
}

/// A server state backed by an in-memory Discord, plus a handle to that Discord.
///
/// Both configured channels are registered on the fake — the equivalent of a bot that really was
/// invited — so tests exercise a correctly deployed server. A test that wants the
/// misconfiguration builds its own [`FakeDiscord`] and leaves the channel out.
#[must_use]
pub fn state() -> (AppState, Arc<FakeDiscord>) {
    let (state, discord, _elevenlabs) = state_parts();
    (state, discord)
}

/// The same server, plus a handle to its in-memory ElevenLabs.
#[must_use]
pub fn state_parts() -> (AppState, Arc<FakeDiscord>, Arc<FakeElevenLabs>) {
    state_from_toml(&config_toml())
}

/// A server built from arbitrary configuration text, for the tests that need a server that is
/// deliberately configured wrong.
///
/// # Panics
///
/// Panics if `text` is not a valid configuration, which is a failure of the test that wrote it.
#[must_use]
pub fn state_from_toml(text: &str) -> (AppState, Arc<FakeDiscord>, Arc<FakeElevenLabs>) {
    let (state, discord, elevenlabs, _store) = state_pieces(text, Arc::new(FakeStore::new()));
    (state, discord, elevenlabs)
}

/// The same server, plus a handle to its in-memory store.
///
/// A separate accessor rather than a fourth element on [`state_from_toml`] so the six existing
/// call sites keep working; a test that cares about durable state asks for it.
#[must_use]
pub fn state_with_store() -> (AppState, Arc<FakeDiscord>, Arc<FakeStore>) {
    let store = Arc::new(FakeStore::new());
    let (state, discord, _elevenlabs, _erased) = state_pieces(&config_toml(), store.clone());
    (state, discord, store)
}

/// A server with its store AND its ElevenLabs stand-in.
///
/// Read-aloud is the first feature that needs both at once: the act is "speak this message, then
/// archive it", and proving it means asking the vendor what it was told and asking the store what
/// was dismissed.
#[must_use]
pub fn state_with_store_and_voice() -> (
    AppState,
    Arc<FakeDiscord>,
    Arc<FakeStore>,
    Arc<FakeElevenLabs>,
) {
    let store = Arc::new(FakeStore::new());
    let (state, discord, elevenlabs, _erased) = state_pieces(&config_toml(), store.clone());
    (state, discord, store, elevenlabs)
}

/// A server built from arbitrary configuration text, plus a handle to its in-memory store.
///
/// The pair [`state_with_store`] and [`state_from_toml`] each give up one of the two things a
/// test of a CONFIGURED durable feature needs — `#46 conversation-replay` is the first, since
/// `[replay]` is off by default and a test of the on case has to set it and then write turns.
#[must_use]
pub fn state_with_store_from_toml(text: &str) -> (AppState, Arc<FakeDiscord>, Arc<FakeStore>) {
    let store = Arc::new(FakeStore::new());
    let (state, discord, _elevenlabs, _erased) = state_pieces(text, store.clone());
    (state, discord, store)
}

/// A server whose store is one the caller built — a [`FakeStore`] primed to fail, a
/// [`crate::store::disabled::DisabledStore`], or a real SQLite file.
#[must_use]
pub fn state_with(store: Arc<dyn crate::store::StateStore>) -> (AppState, Arc<FakeDiscord>) {
    let (state, discord, _elevenlabs, _store) = state_pieces(&config_toml(), store);
    (state, discord)
}

fn state_pieces(
    text: &str,
    store: Arc<dyn crate::store::StateStore>,
) -> (
    AppState,
    Arc<FakeDiscord>,
    Arc<FakeElevenLabs>,
    Arc<dyn crate::store::StateStore>,
) {
    state_pieces_with(
        text,
        store,
        Arc::new(crate::summarize::fake::FakeSummarizer::new()),
    )
}

/// A server whose summariser is one the caller can count calls on.
#[must_use]
pub fn state_with_summarizer(
    text: &str,
) -> (
    AppState,
    Arc<FakeDiscord>,
    Arc<FakeStore>,
    Arc<crate::summarize::fake::FakeSummarizer>,
) {
    let store = Arc::new(FakeStore::new());
    let summarizer = Arc::new(crate::summarize::fake::FakeSummarizer::new());
    let (state, discord, _elevenlabs, _erased) =
        state_pieces_with(text, store.clone(), summarizer.clone());
    (state, discord, store, summarizer)
}

/// A server whose summariser is one the caller built, plus its store.
///
/// The counterpart of [`state_with`] for the OTHER pluggable half. [`state_with_summarizer`] hands
/// back a summariser that counts, which is all `#49 cached-summaries` needed; a summariser that
/// talks to a vendor is built by the test, out of the vendor stand-in the test also keeps a handle
/// to, so this takes one rather than making one.
#[must_use]
pub fn state_with_backend(
    text: &str,
    summarizer: Arc<dyn crate::summarize::Summarizer>,
) -> (AppState, Arc<FakeDiscord>, Arc<FakeStore>) {
    let store = Arc::new(FakeStore::new());
    let (state, discord, _elevenlabs, _erased) = state_pieces_with(text, store.clone(), summarizer);
    (state, discord, store)
}

fn state_pieces_with(
    text: &str,
    store: Arc<dyn crate::store::StateStore>,
    summarizer: Arc<dyn crate::summarize::Summarizer>,
) -> (
    AppState,
    Arc<FakeDiscord>,
    Arc<FakeElevenLabs>,
    Arc<dyn crate::store::StateStore>,
) {
    crate::egress::confine_to_loopback();
    let fake = Arc::new(FakeDiscord::new());
    fake.register_channel(&crate::model::ChannelId(READ_CHANNEL.to_owned()));
    fake.register_channel(&crate::model::ChannelId(WRITE_CHANNEL.to_owned()));
    let elevenlabs = Arc::new(FakeElevenLabs::new());
    let config = Config::from_toml_and_env(text, &BTreeMap::new()).expect("test config is valid");
    let config_for_version = config.summaries.clone();
    let conversation = crate::conversation::provider(
        &config.conversation,
        elevenlabs.clone(),
        config.elevenlabs.clone(),
    );
    let speech: Arc<dyn crate::speech::SpeechProvider> = match config.read_aloud.backend {
        crate::config::ReadAloudBackend::Browser => Arc::new(crate::speech::BrowserSpeech),
        crate::config::ReadAloudBackend::Conversation => {
            Arc::new(crate::speech::ConversationSpeech::new(
                &config.conversation,
                config.read_aloud.websocket_url.as_deref(),
            ))
        }
        crate::config::ReadAloudBackend::ElevenLabs => {
            Arc::new(crate::elevenlabs::speech::ElevenLabsSpeech::new(
                elevenlabs.clone(),
                config.elevenlabs.clone(),
            ))
        }
    };
    // The fake answers for the deployment's one provider, recorded under that provider's own
    // namespace exactly as `main` records a real client's.
    let providers = Arc::new(crate::providers::ChatRouter::single(
        fake.clone(),
        &config.providers[0].namespace(),
        config.providers[0].live_poll_seconds,
    ));
    // As `main` builds them: the message cache's handle first, attached to the hub before anything
    // can publish. `#208 persistent-cache`.
    let message_cache = Arc::new(crate::cache::Sink::off());
    let live = Arc::new(crate::live::LiveHub::new());
    live.attach_cache(Arc::clone(&message_cache));
    // As `main` attaches it: the poller colours threads as it meets them. `#427 server-thread-hues`.
    live.attach_store(Arc::clone(&store));
    // `#331 resume-on-websocket`. Nothing asked yet: a test that needs the bridge's answer asks for
    // it (`BridgeHello::ask`), so the moment it arrives is the test's, not a background task's.
    let voice_bridge = Arc::new(crate::voice_bridge::BridgeHello::for_config(&config));
    let state = AppState {
        config: Arc::new(config),
        chat: providers.clone(),
        providers,
        ranker: Arc::new(LexicalRanker),
        agent: Arc::new(NoAgentBackend),
        elevenlabs: elevenlabs.clone(),
        conversation,
        speech,
        store: Arc::clone(&store),
        // A real hub, never a stub: a test drives `live::poll_once` or `AppState::live.publish`
        // directly and then reads the SSE route, which is the whole ingestion path minus the
        // timer. Sleeping for a poll interval would make the suite slow and flaky for no extra
        // coverage — the timer is `live::poll_forever`, and what it does per tick is `poll_once`.
        added_channels: Arc::new(std::sync::RwLock::new(Vec::new())),
        hidden_channels: Arc::new(std::sync::RwLock::new(std::collections::BTreeSet::new())),
        channel_registration_lock: Arc::new(tokio::sync::Mutex::new(())),
        voice_health_budget: Arc::new(crate::voice_health::LogBudget::new()),
        voice_end_budget: Arc::new(crate::voice_health::LogBudget::new()),
        pairing: Arc::new(crate::pairing::Pairing::new()),
        headings: Arc::new(crate::heading::HeadingTags::new()),
        speech_tickets: Arc::new(crate::speech_tickets::SpeechTickets::new()),
        read_ahead: Arc::new(crate::read_ahead::ReadAhead::new()),
        recent_view_shadow: Arc::new(crate::recent_view::Shadow::new()),
        agent_override: Arc::new(crate::voice_prompt::OverrideAnswers::new()),
        voice_bridge,
        message_cache,
        // Numbered from one, so a test can name the sends it makes; a server numbers from its clock.
        post_gate: Arc::new(crate::post_gate::PostGate::with_ttl_and_numbering(
            crate::post_gate::PROPOSAL_TTL,
            0,
        )),
        post_lanes: Arc::new(crate::post_lanes::PostLanes::new()),
        reply_reads: Arc::new(crate::reply_read::ReplyReads::new()),
        spoken_names: Arc::new(crate::speakable::SharedNames::new()),
        live,
        summarizer: Arc::clone(&summarizer),
        // Taken from the summariser rather than named here, exactly as `main` takes it, so a test
        // that substitutes a different backend gets that backend's cache key rather than one this
        // scaffolding invented.
        summary_version: crate::summarize::policy_version_for(
            &config_for_version,
            summarizer.as_ref(),
        )
        .into(),
        summary_counters: Arc::new(crate::summarize::counters::SummaryCounters::new()),
        thread_names: Arc::new(crate::summarize::threads::ThreadNames::new()),
    };
    state
        .message_cache
        .attach_summaries(crate::cache::orphans::Summaries::of(&state));
    (state, fake, elevenlabs, store)
}

/// A chat provider that takes its time, ONE READ AT A TIME: the shape of a bridge with a single
/// command slot in front of a slow service. `#210 voice-catchup-speed`.
///
/// Every read of a channel's messages queues for the one slot, then waits its delay, then answers
/// from the [`FakeDiscord`] it wraps. A channel can be stalled on its own, to stand for the one
/// command that hangs for a minute. Time is tokio's, so a test that pauses the clock measures
/// these waits exactly and spends none of them.
///
/// By default a read answers with the channel as it stands when the delay ENDS. A real provider
/// answers with a picture taken somewhere inside it, and a read that lands after a change it does
/// not show is the case [`SlowChat::answering_as_of_the_start`] stands for.
pub struct SlowChat {
    inner: Arc<FakeDiscord>,
    slot: tokio::sync::Semaphore,
    delay: Duration,
    stalled: Mutex<BTreeMap<crate::model::ChannelId, Duration>>,
    reads: std::sync::atomic::AtomicUsize,
    as_of_start: bool,
}

impl SlowChat {
    /// Wrap `inner`, every read taking `delay` once it holds the slot.
    #[must_use]
    pub fn new(inner: Arc<FakeDiscord>, delay: Duration) -> Self {
        Self {
            inner,
            slot: tokio::sync::Semaphore::new(1),
            delay,
            stalled: Mutex::new(BTreeMap::new()),
            reads: std::sync::atomic::AtomicUsize::new(0),
            as_of_start: false,
        }
    }

    /// Answer every read with the channel as it stood when the read took the slot, then wait the
    /// delay: whatever changes during the delay is missing from the answer that lands after it.
    #[must_use]
    pub fn answering_as_of_the_start(mut self) -> Self {
        self.as_of_start = true;
        self
    }

    /// Make every read of `channel` take `delay` instead, until [`SlowChat::release`].
    pub fn stall(&self, channel: &crate::model::ChannelId, delay: Duration) {
        lock(&self.stalled).insert(channel.clone(), delay);
    }

    /// Give `channel` the ordinary delay again.
    pub fn release(&self, channel: &crate::model::ChannelId) {
        lock(&self.stalled).remove(channel);
    }

    /// How many reads were asked of this provider, queued ones included.
    #[must_use]
    pub fn reads(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Hold the one slot for a read of `channel`, and answer it with what `read` gives: taken
    /// before or after the delay, as this provider was built to.
    async fn read<T, F>(
        &self,
        channel: &crate::model::ChannelId,
        read: F,
    ) -> Result<T, crate::chat::ChatError>
    where
        F: std::future::Future<Output = Result<T, crate::chat::ChatError>>,
    {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _turn = self
            .slot
            .acquire()
            .await
            .map_err(|_| crate::chat::ChatError::Transport("the slot closed".to_owned()))?;
        let delay = lock(&self.stalled)
            .get(channel)
            .copied()
            .unwrap_or(self.delay);
        if self.as_of_start {
            let answer = read.await;
            tokio::time::sleep(delay).await;
            answer
        } else {
            tokio::time::sleep(delay).await;
            read.await
        }
    }
}

fn lock<T>(cell: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    cell.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[async_trait::async_trait]
impl crate::chat::ChatClient for SlowChat {
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    async fn identity(&self) -> Result<crate::chat::ChatIdentity, crate::chat::ChatError> {
        self.inner.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &crate::model::ChannelId,
        limit: u16,
        before: Option<&crate::model::MessageId>,
        after: Option<&crate::model::MessageId>,
    ) -> Result<Vec<crate::model::Message>, crate::chat::ChatError> {
        self.read(
            channel,
            self.inner.fetch_page(channel, limit, before, after),
        )
        .await
    }

    async fn fetch_recent(
        &self,
        channel: &crate::model::ChannelId,
        limit: u16,
    ) -> Result<Vec<crate::model::Message>, crate::chat::ChatError> {
        self.read(channel, self.inner.fetch_recent(channel, limit))
            .await
    }

    async fn post_message(
        &self,
        channel: &crate::model::ChannelId,
        content: &str,
        reply_to: Option<&crate::model::MessageId>,
    ) -> Result<crate::model::Message, crate::chat::ChatError> {
        self.inner.post_message(channel, content, reply_to).await
    }
}

/// A Discord-protocol provider with timelines, answered from the [`FakeDiscord`] it wraps. `#208
/// persistent-cache`: what a test of the message cache reads `/timeline` through, as Discord's own
/// API or as a bridge, by the deployment's `thread_api`.
///
/// Every view's newest page is the channel's newest messages and a `before` page the ones older
/// than the cursor, with `has_more` saying whether older ones exist: the over-fetch a real
/// provider does. A thread is the message it is named after and the messages that reply to it,
/// out of a channel of fewer than a hundred messages.
/// A forward read answers the messages after the cursor, as the delta [`TimelineChat::delta`]
/// says, and every newest page or delta hands back its newest id as its own forward cursor.
pub struct TimelineChat {
    inner: Arc<FakeDiscord>,
    as_of: Mutex<Option<String>>,
    delta: Mutex<crate::threads::TimelineDelta>,
    hold: Mutex<Option<HoldGate>>,
    hold_recent: Mutex<Option<HoldGate>>,
}

/// The provider's side of a [`HeldRead`].
struct HoldGate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

/// One `/timeline` read held at the provider, from [`TimelineChat::hold_next_timeline`]: it has
/// taken its answer from the channel as it stood and waits before handing it back, so a test can
/// change the world underneath a read that is in flight.
pub struct HeldRead {
    /// Answers once the read has taken its answer and is waiting.
    pub entered: tokio::sync::oneshot::Receiver<()>,
    /// Send to it, or drop it, to let the read hand its answer back.
    pub release: tokio::sync::oneshot::Sender<()>,
}

impl TimelineChat {
    /// Answer from `inner`.
    #[must_use]
    pub fn new(inner: Arc<FakeDiscord>) -> Self {
        Self {
            inner,
            as_of: Mutex::new(None),
            delta: Mutex::new(crate::threads::TimelineDelta::default()),
            hold: Mutex::new(None),
            hold_recent: Mutex::new(None),
        }
    }

    /// Hold the next `/timeline` read once it has taken its answer, until the returned
    /// [`HeldRead::release`] is sent to or dropped.
    #[must_use]
    pub fn hold_next_timeline(&self) -> HeldRead {
        Self::hold_next(&self.hold)
    }

    /// Hold the next read of a channel's most recent messages (`fetch_recent_dated`) the same way.
    #[must_use]
    pub fn hold_next_recent(&self) -> HeldRead {
        Self::hold_next(&self.hold_recent)
    }

    fn hold_next(slot: &Mutex<Option<HoldGate>>) -> HeldRead {
        let (entered_tx, entered) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        *lock(slot) = Some(HoldGate {
            entered: entered_tx,
            release: release_rx,
        });
        HeldRead { entered, release }
    }

    /// Wait here if a test asked for this read to be held.
    async fn held(slot: &Mutex<Option<HoldGate>>) {
        let gate = lock(slot).take();
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            let _ = gate.release.await;
        }
    }

    /// Date every page and recent read `as_of` from now on, or stop dating them.
    pub fn set_as_of(&self, as_of: Option<String>) {
        *lock(&self.as_of) = as_of;
    }

    /// What every forward read says about the changes it carries from now on: whether it is
    /// complete, and what it reports deleted.
    pub fn set_delta(&self, delta: crate::threads::TimelineDelta) {
        *lock(&self.delta) = delta;
    }

    fn cursor(raw: &str) -> Result<crate::model::MessageId, crate::chat::ChatError> {
        let id = crate::model::MessageId(raw.to_owned());
        id.numeric()
            .map(|_| id.clone())
            .ok_or_else(|| crate::chat::ChatError::Refused("not this provider's cursor".to_owned()))
    }

    fn in_thread(message: &crate::model::Message, thread: &str) -> bool {
        message.id.as_str() == thread
            || message
                .reply_to
                .as_ref()
                .is_some_and(|parent| parent.as_str() == thread)
    }
}

#[async_trait::async_trait]
impl crate::chat::ChatClient for TimelineChat {
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn supports_threading(&self) -> bool {
        true
    }

    async fn identity(&self) -> Result<crate::chat::ChatIdentity, crate::chat::ChatError> {
        self.inner.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &crate::model::ChannelId,
        limit: u16,
        before: Option<&crate::model::MessageId>,
        after: Option<&crate::model::MessageId>,
    ) -> Result<Vec<crate::model::Message>, crate::chat::ChatError> {
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn fetch_recent_dated(
        &self,
        channel: &crate::model::ChannelId,
        limit: u16,
    ) -> Result<crate::chat::RecentRead, crate::chat::ChatError> {
        let messages = self.inner.fetch_recent(channel, limit).await?;
        Self::held(&self.hold_recent).await;
        Ok(crate::chat::RecentRead {
            messages,
            as_of: lock(&self.as_of).clone(),
            exhausted: false,
            notice: None,
        })
    }

    async fn fetch_timeline(
        &self,
        channel: &crate::model::ChannelId,
        request: &crate::threads::TimelineRequest,
    ) -> Result<crate::threads::TimelinePage, crate::chat::ChatError> {
        use crate::threads::{TimelinePage, TimelineView};
        request.one_direction()?;
        if request.view == TimelineView::Threads {
            return Ok(TimelinePage::default());
        }
        let limit = usize::from(request.limit.clamp(1, 99));
        let thread = request.thread_id.as_deref();
        // A thread is picked out of the channel's newest hundred: a test channel holds fewer.
        let ask = if thread.is_some() {
            100
        } else {
            u16::try_from(limit + 1).unwrap_or(100)
        };
        let keep = |messages: Vec<crate::model::Message>| -> Vec<crate::model::Message> {
            messages
                .into_iter()
                .filter(|m| thread.is_none_or(|thread| Self::in_thread(m, thread)))
                .collect()
        };
        if let Some(after) = &request.after {
            let after = Self::cursor(after)?;
            let messages = keep(
                self.inner
                    .fetch_page(channel, ask, None, Some(&after))
                    .await?,
            );
            let newest = messages.last().map_or(after, |m| m.id.clone());
            return Ok(TimelinePage {
                messages,
                next_after: Some(newest.as_str().to_owned()),
                delta: Some(lock(&self.delta).clone()),
                as_of: lock(&self.as_of).clone(),
                ..TimelinePage::default()
            });
        }
        let before = request.before.as_deref().map(Self::cursor).transpose()?;
        let mut messages = keep(
            self.inner
                .fetch_page(channel, ask, before.as_ref(), None)
                .await?,
        );
        let has_more = messages.len() > limit;
        if has_more {
            messages.drain(..messages.len() - limit);
        }
        Self::held(&self.hold).await;
        Ok(TimelinePage {
            next_before: has_more
                .then(|| messages.first().map(|m| m.id.as_str().to_owned()))
                .flatten(),
            next_after: request
                .before
                .is_none()
                .then(|| messages.last().map(|m| m.id.as_str().to_owned()))
                .flatten(),
            messages,
            has_more,
            as_of: lock(&self.as_of).clone(),
            ..TimelinePage::default()
        })
    }

    async fn fetch_thread_message(
        &self,
        _channel: &crate::model::ChannelId,
        thread_id: &str,
        message_id: &crate::model::MessageId,
    ) -> Result<crate::model::Message, crate::chat::ChatError> {
        self.inner
            .message(message_id)
            .filter(|m| Self::in_thread(m, thread_id))
            .ok_or_else(|| crate::chat::ChatError::Refused("not in that thread".to_owned()))
    }

    async fn post_message(
        &self,
        channel: &crate::model::ChannelId,
        content: &str,
        reply_to: Option<&crate::model::MessageId>,
    ) -> Result<crate::model::Message, crate::chat::ChatError> {
        self.inner.post_message(channel, content, reply_to).await
    }
}

/// One call [`CountingChat`] passed on, or answered with a scripted failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountedCall {
    /// The `ChatClient` method, by name: `fetch_timeline`, `fetch_page`, `post_message`, ...
    pub method: &'static str,
    /// The channel the call named, when it named one.
    pub channel: Option<crate::model::ChannelId>,
    /// The timeline view, for `fetch_timeline` and `fetch_timeline_since`.
    pub view: Option<crate::threads::TimelineView>,
    /// The thread, for a call scoped to one.
    pub thread: Option<String>,
    /// Why it was made: the [`crate::read_reason::ReadReason`] around the call when it was made.
    pub reason: crate::read_reason::ReadReason,
}

/// A failure [`CountingChat`] answers in place of the provider, once.
struct ScriptedFailure {
    method: &'static str,
    view: Option<crate::threads::TimelineView>,
    error: crate::chat::ChatError,
}

/// A chat provider that COUNTS every upstream call made through it: the common fixture of the
/// remote-ops audit (2026-10-13, section 5), so a test can assert how many calls one user-visible
/// action costs, per method, channel, timeline view and read reason.
///
/// Wraps any [`crate::chat::ChatClient`] and forwards every method to it, the capability flags
/// included, so the wrapped provider behaves exactly as it would unwrapped. Each async method (the
/// ones that may reach a provider) is recorded as a [`CountedCall`] when it is made, before it
/// answers. Optionally every such call waits a delay first ([`CountingChat::with_delay`]; tokio's
/// clock, so a paused test spends none of it), and a call can be scripted to fail
/// ([`CountingChat::fail_next`]), still counted, without reaching the wrapped provider.
pub struct CountingChat {
    inner: Arc<dyn crate::chat::ChatClient>,
    calls: Mutex<Vec<CountedCall>>,
    failures: Mutex<Vec<ScriptedFailure>>,
    delay: Duration,
}

impl CountingChat {
    /// Count every call made to `inner`, answering at once.
    #[must_use]
    pub fn new(inner: Arc<dyn crate::chat::ChatClient>) -> Self {
        Self {
            inner,
            calls: Mutex::new(Vec::new()),
            failures: Mutex::new(Vec::new()),
            delay: Duration::ZERO,
        }
    }

    /// Make every counted call wait `delay` before it answers.
    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Answer the next call of `method` (of `view`, when one is given) with `error`, without
    /// reaching the wrapped provider. Each scripted failure is used once, oldest first.
    pub fn fail_next(
        &self,
        method: &'static str,
        view: Option<crate::threads::TimelineView>,
        error: crate::chat::ChatError,
    ) {
        lock(&self.failures).push(ScriptedFailure {
            method,
            view,
            error,
        });
    }

    /// Every call made so far, oldest first.
    #[must_use]
    pub fn calls(&self) -> Vec<CountedCall> {
        lock(&self.calls).clone()
    }

    /// How many calls of `method` were made, to any channel.
    #[must_use]
    pub fn count(&self, method: &str) -> usize {
        lock(&self.calls)
            .iter()
            .filter(|call| call.method == method)
            .count()
    }

    /// How many `fetch_timeline` reads of `view` were made in `channel`.
    #[must_use]
    pub fn timeline_reads(
        &self,
        channel: &crate::model::ChannelId,
        view: crate::threads::TimelineView,
    ) -> usize {
        lock(&self.calls)
            .iter()
            .filter(|call| {
                call.method == "fetch_timeline"
                    && call.channel.as_ref() == Some(channel)
                    && call.view == Some(view)
            })
            .count()
    }

    /// Record `call`, wait the delay, and answer with a scripted failure or with `forward`.
    async fn counted<T, F>(
        &self,
        call: CountedCall,
        forward: F,
    ) -> Result<T, crate::chat::ChatError>
    where
        F: std::future::Future<Output = Result<T, crate::chat::ChatError>>,
    {
        let scripted = {
            let mut failures = lock(&self.failures);
            failures
                .iter()
                .position(|failure| {
                    failure.method == call.method
                        && failure.view.is_none_or(|view| call.view == Some(view))
                })
                .map(|at| failures.remove(at).error)
        };
        lock(&self.calls).push(call);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        match scripted {
            Some(error) => Err(error),
            None => forward.await,
        }
    }
}

/// A [`CountedCall`] of `method` in `channel`.
fn call_in(method: &'static str, channel: &crate::model::ChannelId) -> CountedCall {
    CountedCall {
        method,
        channel: Some(channel.clone()),
        view: None,
        thread: None,
        reason: crate::read_reason::ReadReason::current(),
    }
}

/// A [`CountedCall`] of `method` that names no channel.
fn call_without_channel(method: &'static str) -> CountedCall {
    CountedCall {
        method,
        channel: None,
        view: None,
        thread: None,
        reason: crate::read_reason::ReadReason::current(),
    }
}

/// A [`CountedCall`] of `method` on `request`'s view and thread in `channel`.
fn timeline_call(
    method: &'static str,
    channel: &crate::model::ChannelId,
    request: &crate::threads::TimelineRequest,
) -> CountedCall {
    CountedCall {
        view: Some(request.view),
        thread: request.thread_id.clone(),
        ..call_in(method, channel)
    }
}

/// A [`CountedCall`] of `method` in `thread` of `channel`.
fn thread_call(
    method: &'static str,
    channel: &crate::model::ChannelId,
    thread: &str,
) -> CountedCall {
    CountedCall {
        thread: Some(thread.to_owned()),
        ..call_in(method, channel)
    }
}

#[async_trait::async_trait]
impl crate::chat::ChatClient for CountingChat {
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn claims_source(&self, source: &str) -> crate::chat::SourceClaim {
        self.inner.claims_source(source)
    }

    fn self_author_id(&self) -> Option<String> {
        self.inner.self_author_id()
    }

    fn owner_author_id(&self) -> Option<String> {
        self.inner.owner_author_id()
    }

    fn upstream_url(
        &self,
        channel: &crate::model::ChannelId,
        id: &crate::model::MessageId,
        thread: Option<&crate::threads::MessageThread>,
    ) -> Option<String> {
        self.inner.upstream_url(channel, id, thread)
    }

    fn supports_threading(&self) -> bool {
        self.inner.supports_threading()
    }

    fn replies_in_thread(&self) -> bool {
        self.inner.replies_in_thread()
    }

    async fn fetch_timeline(
        &self,
        channel: &crate::model::ChannelId,
        request: &crate::threads::TimelineRequest,
    ) -> Result<crate::threads::TimelinePage, crate::chat::ChatError> {
        self.counted(
            timeline_call("fetch_timeline", channel, request),
            self.inner.fetch_timeline(channel, request),
        )
        .await
    }

    async fn fetch_timeline_since(
        &self,
        channel: &crate::model::ChannelId,
        request: &crate::threads::TimelineRequest,
        since: &crate::timeline_forward::GenericPosition,
    ) -> Result<crate::timeline_forward::GenericDelta, crate::chat::ChatError> {
        self.counted(
            timeline_call("fetch_timeline_since", channel, request),
            self.inner.fetch_timeline_since(channel, request, since),
        )
        .await
    }

    async fn post_in_thread(
        &self,
        channel: &crate::model::ChannelId,
        thread_id: &str,
        content: &str,
        reply_to: Option<&crate::model::MessageId>,
    ) -> Result<crate::model::Message, crate::chat::ChatError> {
        self.counted(
            thread_call("post_in_thread", channel, thread_id),
            self.inner
                .post_in_thread(channel, thread_id, content, reply_to),
        )
        .await
    }

    async fn fetch_thread_message(
        &self,
        channel: &crate::model::ChannelId,
        thread_id: &str,
        message_id: &crate::model::MessageId,
    ) -> Result<crate::model::Message, crate::chat::ChatError> {
        self.counted(
            thread_call("fetch_thread_message", channel, thread_id),
            self.inner
                .fetch_thread_message(channel, thread_id, message_id),
        )
        .await
    }

    fn supports_channel_registration(&self) -> bool {
        self.inner.supports_channel_registration()
    }

    async fn register_channel(
        &self,
        source: &str,
        label: &str,
    ) -> Result<crate::chat::RegisteredChannel, crate::chat::ChatError> {
        self.counted(
            call_without_channel("register_channel"),
            self.inner.register_channel(source, label),
        )
        .await
    }

    async fn unregister_channel(
        &self,
        channel: &crate::model::ChannelId,
    ) -> Result<(), crate::chat::ChatError> {
        self.counted(
            call_in("unregister_channel", channel),
            self.inner.unregister_channel(channel),
        )
        .await
    }

    fn supports_channel_discovery(&self) -> bool {
        self.inner.supports_channel_discovery()
    }

    fn forget_channel_directory(&self) {
        self.inner.forget_channel_directory();
    }

    fn added_channels_writable(&self) -> Option<bool> {
        self.inner.added_channels_writable()
    }

    async fn discover_channels(
        &self,
        request: &crate::directory::DirectoryRequest,
    ) -> Result<crate::directory::DirectoryPage, crate::chat::ChatError> {
        self.counted(
            call_without_channel("discover_channels"),
            self.inner.discover_channels(request),
        )
        .await
    }

    fn supports_upstream_read_mark(&self) -> bool {
        self.inner.supports_upstream_read_mark()
    }

    async fn identity(&self) -> Result<crate::chat::ChatIdentity, crate::chat::ChatError> {
        self.counted(call_without_channel("identity"), self.inner.identity())
            .await
    }

    async fn fetch_page(
        &self,
        channel: &crate::model::ChannelId,
        limit: u16,
        before: Option<&crate::model::MessageId>,
        after: Option<&crate::model::MessageId>,
    ) -> Result<Vec<crate::model::Message>, crate::chat::ChatError> {
        self.counted(
            call_in("fetch_page", channel),
            self.inner.fetch_page(channel, limit, before, after),
        )
        .await
    }

    async fn probe_channel(
        &self,
        channel: &crate::model::ChannelId,
        limit: u16,
    ) -> Result<Vec<crate::model::Message>, crate::chat::ChatError> {
        self.counted(
            call_in("probe_channel", channel),
            self.inner.probe_channel(channel, limit),
        )
        .await
    }

    async fn fetch_recent(
        &self,
        channel: &crate::model::ChannelId,
        limit: u16,
    ) -> Result<Vec<crate::model::Message>, crate::chat::ChatError> {
        self.counted(
            call_in("fetch_recent", channel),
            self.inner.fetch_recent(channel, limit),
        )
        .await
    }

    async fn fetch_recent_dated(
        &self,
        channel: &crate::model::ChannelId,
        limit: u16,
    ) -> Result<crate::chat::RecentRead, crate::chat::ChatError> {
        self.counted(
            call_in("fetch_recent_dated", channel),
            self.inner.fetch_recent_dated(channel, limit),
        )
        .await
    }

    fn polls_thread_replies(&self, channel: &crate::model::ChannelId) -> bool {
        self.inner.polls_thread_replies(channel)
    }

    async fn fetch_live(
        &self,
        channel: &crate::model::ChannelId,
        limit: u16,
        after: Option<&crate::model::MessageId>,
    ) -> Result<crate::chat::LiveRead, crate::chat::ChatError> {
        self.counted(
            call_in("fetch_live", channel),
            self.inner.fetch_live(channel, limit, after),
        )
        .await
    }

    fn read_caps(&self, channel: &crate::model::ChannelId) -> crate::chat::ReadCaps {
        self.inner.read_caps(channel)
    }

    async fn fetch_range(
        &self,
        channel: &crate::model::ChannelId,
        scope: &crate::chat::ReadScope,
        range: crate::chat::TimeRange,
        order: crate::chat::RangeOrder,
        limit: u16,
        cursor: Option<&str>,
    ) -> Result<crate::chat::RangeRead, crate::chat::ChatError> {
        let call = match scope {
            crate::chat::ReadScope::Thread(thread) => thread_call("fetch_range", channel, thread),
            _ => call_in("fetch_range", channel),
        };
        self.counted(
            call,
            self.inner
                .fetch_range(channel, scope, range, order, limit, cursor),
        )
        .await
    }

    /// Counted as what it costs upstream, as `ProfiledClient` times it: a native lookup is one
    /// `fetch_messages`; an emulated one runs the trait's default here, so each read it is made of
    /// is counted as itself. remote-ops-audit W11 `#287 bulk-reads`.
    async fn fetch_messages(
        &self,
        channel: &crate::model::ChannelId,
        thread: Option<&str>,
        ids: &[crate::model::MessageId],
    ) -> Result<Vec<crate::chat::Lookup>, crate::chat::ChatError> {
        match self.inner.read_caps(channel).by_ids {
            crate::chat::Support::Native => {
                let call = match thread {
                    Some(thread) => thread_call("fetch_messages", channel, thread),
                    None => call_in("fetch_messages", channel),
                };
                self.counted(call, self.inner.fetch_messages(channel, thread, ids))
                    .await
            }
            crate::chat::Support::Emulated => {
                crate::chat::emulated_messages(self, channel, thread, ids).await
            }
        }
    }

    /// Counted as what it costs upstream, as `ProfiledClient` times it: the channels whose heads
    /// the provider reads itself are one `channel_heads` (naming no single channel); for the rest
    /// the trait's default runs here, so each `fetch_recent` it is made of is counted as itself. A
    /// scripted failure of `channel_heads` is every native channel's answer. remote-ops-audit W12
    /// `#287 bulk-reads`.
    async fn channel_heads(
        &self,
        channels: &[crate::model::ChannelId],
    ) -> crate::chat::HeadAnswers {
        let (native, emulated): (Vec<usize>, Vec<usize>) = (0..channels.len()).partition(|at| {
            self.inner.read_caps(&channels[*at]).heads == crate::chat::Support::Native
        });
        let pick = |positions: &[usize]| -> Vec<crate::model::ChannelId> {
            positions.iter().map(|at| channels[*at].clone()).collect()
        };
        let (native_channels, emulated_channels) = (pick(&native), pick(&emulated));
        let native_answers = if native_channels.is_empty() {
            Vec::new()
        } else {
            match self
                .counted(call_without_channel("channel_heads"), async {
                    Ok(self.inner.channel_heads(&native_channels).await)
                })
                .await
            {
                Ok(answers) => answers,
                Err(error) => native_channels
                    .iter()
                    .map(|channel| (channel.clone(), Err(error.clone())))
                    .collect(),
            }
        };
        let emulated_answers = crate::chat::emulated_heads(self, &emulated_channels).await;
        crate::chat::heads_in_order(
            channels,
            vec![(native, native_answers), (emulated, emulated_answers)],
        )
    }

    /// Counted as what it costs upstream, as `ProfiledClient` times it: the channels the provider
    /// reads together itself are one `fetch_recent_many` (naming no single channel); for the rest
    /// the trait's default runs here, so each `fetch_recent_dated` it is made of is counted as
    /// itself. A scripted failure of `fetch_recent_many` is every native ask's answer.
    /// remote-ops-audit W13 `#287 bulk-reads`.
    async fn fetch_recent_many(
        &self,
        asks: &[(crate::model::ChannelId, crate::chat::RecentAsk)],
    ) -> crate::chat::RecentAnswers {
        let (native, emulated): (Vec<usize>, Vec<usize>) = (0..asks.len()).partition(|at| {
            self.inner.read_caps(&asks[*at].0).many_channels == crate::chat::Support::Native
        });
        let pick = |positions: &[usize]| -> Vec<(crate::model::ChannelId, crate::chat::RecentAsk)> {
            positions.iter().map(|at| asks[*at].clone()).collect()
        };
        let (native_asks, emulated_asks) = (pick(&native), pick(&emulated));
        let native_answers = if native_asks.is_empty() {
            Vec::new()
        } else {
            match self
                .counted(call_without_channel("fetch_recent_many"), async {
                    Ok(self.inner.fetch_recent_many(&native_asks).await)
                })
                .await
            {
                Ok(answers) => answers,
                Err(error) => native_asks
                    .iter()
                    .map(|(channel, _)| (channel.clone(), Err(error.clone())))
                    .collect(),
            }
        };
        let emulated_answers = crate::chat::emulated_recent_many(self, &emulated_asks).await;
        let channels: Vec<crate::model::ChannelId> =
            asks.iter().map(|(channel, _)| channel.clone()).collect();
        crate::chat::recent_in_order(
            &channels,
            vec![(native, native_answers), (emulated, emulated_answers)],
        )
    }

    /// Counted as what it costs upstream, as `ProfiledClient` times it: a provider that reads
    /// several threads itself is one `fetch_threads` in the channel; for any other the trait's
    /// default runs here, so each thread view read it is made of is counted as the
    /// `fetch_timeline` it is. A scripted failure of `fetch_threads` is every ask's answer.
    /// Nothing is counted for no asks. remote-ops-audit W14 `#287 bulk-reads`.
    async fn fetch_threads(
        &self,
        channel: &crate::model::ChannelId,
        asks: &[crate::chat::ThreadAsk],
    ) -> crate::chat::ThreadAnswers {
        if asks.is_empty() {
            return Vec::new();
        }
        match self.inner.read_caps(channel).many_threads {
            crate::chat::Support::Native => {
                match self
                    .counted(call_in("fetch_threads", channel), async {
                        Ok(self.inner.fetch_threads(channel, asks).await)
                    })
                    .await
                {
                    Ok(answers) => answers,
                    Err(error) => asks
                        .iter()
                        .map(|ask| (ask.id.clone(), Err(error.clone())))
                        .collect(),
                }
            }
            crate::chat::Support::Emulated => {
                crate::chat::emulated_threads(self, channel, asks).await
            }
        }
    }

    async fn post_message(
        &self,
        channel: &crate::model::ChannelId,
        content: &str,
        reply_to: Option<&crate::model::MessageId>,
    ) -> Result<crate::model::Message, crate::chat::ChatError> {
        self.counted(
            call_in("post_message", channel),
            self.inner.post_message(channel, content, reply_to),
        )
        .await
    }

    fn supports_idempotent_posts(&self) -> bool {
        self.inner.supports_idempotent_posts()
    }

    async fn post_message_keyed(
        &self,
        channel: &crate::model::ChannelId,
        content: &str,
        reply_to: Option<&crate::model::MessageId>,
        key: Option<&str>,
    ) -> Result<crate::model::Message, crate::chat::ChatError> {
        self.counted(
            call_in("post_message_keyed", channel),
            self.inner
                .post_message_keyed(channel, content, reply_to, key),
        )
        .await
    }

    async fn post_in_thread_keyed(
        &self,
        channel: &crate::model::ChannelId,
        thread_id: &str,
        content: &str,
        reply_to: Option<&crate::model::MessageId>,
        key: Option<&str>,
    ) -> Result<crate::model::Message, crate::chat::ChatError> {
        self.counted(
            thread_call("post_in_thread_keyed", channel, thread_id),
            self.inner
                .post_in_thread_keyed(channel, thread_id, content, reply_to, key),
        )
        .await
    }

    fn post_limit(&self, channel: &crate::model::ChannelId) -> crate::chat::PostLimit {
        self.inner.post_limit(channel)
    }

    fn continuation_thread(
        &self,
        channel: &crate::model::ChannelId,
        first: &crate::model::Message,
    ) -> Option<String> {
        self.inner.continuation_thread(channel, first)
    }

    async fn mark_read_upstream(
        &self,
        channel: &crate::model::ChannelId,
        through: &crate::model::MessageId,
    ) -> Result<(), crate::chat::ChatError> {
        self.counted(
            call_in("mark_read_upstream", channel),
            self.inner.mark_read_upstream(channel, through),
        )
        .await
    }

    async fn upstream_read_scope(
        &self,
        channel: &crate::model::ChannelId,
    ) -> Result<crate::chat::UpstreamReadScope, crate::chat::ChatError> {
        self.counted(
            call_in("upstream_read_scope", channel),
            self.inner.upstream_read_scope(channel),
        )
        .await
    }

    fn supports_upstream_clear(&self) -> bool {
        self.inner.supports_upstream_clear()
    }

    async fn clear_upstream_through(
        &self,
        channel: &crate::model::ChannelId,
        through: &crate::model::MessageId,
        dry_run: bool,
    ) -> Result<crate::chat::UpstreamClearOutcome, crate::chat::ChatError> {
        self.counted(
            call_in("clear_upstream_through", channel),
            self.inner.clear_upstream_through(channel, through, dry_run),
        )
        .await
    }
}

/// `state` with its channel windows kept to `timing` rather than the production durations.
/// `#210 voice-catchup-speed`. A fresh hub, so call it before anything is published. The fresh hub
/// offers its changes to the state's own message-cache handle, as the one it replaces did.
#[must_use]
pub fn with_recent_timing(mut state: AppState, timing: crate::recent::Timing) -> AppState {
    let live = crate::live::LiveHub::with_recent(Arc::new(
        crate::recent::RecentWindows::with_timing(timing),
    ));
    live.attach_cache(Arc::clone(&state.message_cache));
    live.attach_store(Arc::clone(&state.store));
    state.live = Arc::new(live);
    state
}

/// A clock a test sets by hand, for the message cache. `#208 persistent-cache`.
///
/// The cache judges age by its own clock: it refuses rows older than its age limit, and a fixture
/// written in 2023 is older than any limit by the system clock. A test places this clock near its
/// fixture's times, and moves it to order one read after another.
#[derive(Debug)]
pub struct ManualClock(std::sync::atomic::AtomicI64);

impl ManualClock {
    /// A clock reading `now_ms`.
    #[must_use]
    pub fn at(now_ms: i64) -> Arc<Self> {
        Arc::new(Self(std::sync::atomic::AtomicI64::new(now_ms)))
    }

    /// Set the clock to `now_ms`.
    pub fn set(&self, now_ms: i64) {
        self.0.store(now_ms, std::sync::atomic::Ordering::SeqCst);
    }

    /// Move the clock forward by `ms`.
    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
    }

    /// What the clock reads.
    #[must_use]
    pub fn now(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl crate::cache::Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now()
    }
}

/// How long [`install_message_cache`] waits for the cache a test closed to let go of its file.
pub const PREVIOUS_CACHE_WAIT: Duration = Duration::from_secs(10);

/// Open a message cache in `dir`, read by `clock` and indexed by the page's own rendering for
/// `state`'s providers, and put it behind `state`'s sink, WITHOUT arming it. `#208
/// persistent-cache`. For a test of the startup guard, which arms it (or does not) through
/// [`AppState::restore_channel_lists`]; every other test wants [`with_message_cache`].
///
/// # Panics
///
/// Panics if the cache cannot be opened, which is a broken test environment, or if a cache a test
/// closed still holds the file after [`PREVIOUS_CACHE_WAIT`].
#[must_use]
pub fn install_message_cache(
    state: &AppState,
    dir: &std::path::Path,
    clock: Arc<ManualClock>,
) -> Arc<crate::cache::MessageCache> {
    let search_text = Arc::new(crate::render::SearchText::from_config(&state.config));
    let options = || crate::cache::CacheOptions {
        // A directory of its own: the cache refuses one that other accounts can enter, which a
        // system temporary directory's children are.
        path: dir.join("message-cache").join("message-cache.sqlite3"),
        store_path: None,
        limits: crate::cache::Limits::default(),
        search_text: search_text.clone(),
        clock: clock.clone(),
    };
    // A test's restart is in-process. Work the last server left running -- a shadow evaluation
    // still reading beside a request it served, on the blocking pool -- can hold that server's
    // cache, and with it the file's lock, for a moment after the test closed it; a real restart's
    // new process finds the lock its old one released on exit. So the next start waits for that
    // last handle to go, up to PREVIOUS_CACHE_WAIT, and then refuses as a server would.
    // `#364 message-cache-test-flakes`.
    let began = std::time::Instant::now();
    let cache = loop {
        match crate::cache::MessageCache::open(options()) {
            Err(crate::cache::CacheError::Locked(_)) if began.elapsed() < PREVIOUS_CACHE_WAIT => {
                std::thread::sleep(Duration::from_millis(1));
            }
            opened => break Arc::new(opened.expect("a message cache opens in a test directory")),
        }
    };
    // As `main` does, for a state built field by field; a no-op for one this module built.
    state
        .message_cache
        .attach_summaries(crate::cache::orphans::Summaries::of(state));
    state.message_cache.install(Arc::clone(&cache));
    cache
}

/// `state` with a message cache open in `dir` behind its sink, armed, and told the channels the
/// server answers for, and every provider's account checked against it: what a server with the
/// cache on looks like once it has started. `#208 persistent-cache`. Call it after the state's
/// providers are final ([`AppState::replace_chat`], [`AppState::replace_providers`]): the cache
/// names each channel under the provider that owns it when this is called, and a provider swapped
/// in later is told to the cache as a change of routes, and checked again.
///
/// The cache is reached again through `state.message_cache.cache()`.
///
/// # Panics
///
/// Panics if the cache cannot be opened, or a provider's account is not checked within twenty
/// seconds: a provider that cannot say which account it is wants
/// [`with_message_cache_unverified`].
pub async fn with_message_cache(
    state: AppState,
    dir: &std::path::Path,
    clock: Arc<ManualClock>,
) -> AppState {
    let state = with_message_cache_unverified(state, dir, clock);
    assert!(
        state
            .message_cache
            .identities_verified(Duration::from_secs(20))
            .await,
        "every provider's account was checked against the new message cache"
    );
    state
}

/// [`with_message_cache`] without waiting for the providers' accounts to be checked: for a test
/// of that check itself. Until it is done, nothing of a provider is filled.
#[must_use]
pub fn with_message_cache_unverified(
    state: AppState,
    dir: &std::path::Path,
    clock: Arc<ManualClock>,
) -> AppState {
    drop(install_message_cache(&state, dir, clock));
    state.message_cache.arm();
    state.refresh_routes();
    state
}

/// A directory that deletes itself, so a test can drive the real SQLite store.
///
/// Hand-rolled rather than a `tempfile` dependency: it is twenty lines, and this crate's
/// dependency list is short on purpose. It is not hardened against a hostile `TMPDIR` — it is
/// test scaffolding, and the process that creates it is the process that removes it.
#[derive(Debug)]
pub struct TempDir(std::path::PathBuf);

impl TempDir {
    /// Create a fresh directory under the system temporary directory.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be created, which is a broken test environment rather than
    /// a test failure.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("vibe-talk-{tag}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&path).expect("a temporary directory can be created");
        Self(path)
    }

    /// The directory itself.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A `tracing` writer that keeps everything in memory.
#[derive(Clone, Debug, Default)]
pub struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedBuffer {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Captures everything logged on THIS THREAD for as long as it is alive.
///
/// Scoped rather than global on purpose: `tracing`'s global subscriber can be set once per
/// process, so a global capture would make the logging tests order-dependent and unable to run
/// beside anything else. `#[tokio::test]` uses a current-thread runtime, so a handler driven
/// through `oneshot` logs on the same thread this guard is installed on.
pub struct LogCapture {
    buffer: SharedBuffer,
    _guard: tracing::subscriber::DefaultGuard,
}

impl LogCapture {
    /// Start capturing at DEBUG and below.
    ///
    /// DEBUG rather than INFO so a test can assert on what is INFO-only by *level filtering*
    /// instead of by absence — see [`Self::info_only`].
    #[must_use]
    pub fn start() -> Self {
        Self::at(tracing::Level::DEBUG)
    }

    /// Start capturing at INFO, which is the level a deployment actually runs at.
    #[must_use]
    pub fn info_only() -> Self {
        Self::at(tracing::Level::INFO)
    }

    fn at(level: tracing::Level) -> Self {
        let buffer = SharedBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_max_level(level)
            .with_ansi(false)
            .without_time()
            .finish();
        Self {
            buffer,
            _guard: tracing::subscriber::set_default(subscriber),
        }
    }

    /// Everything captured so far.
    ///
    /// # Panics
    ///
    /// Panics if the captured bytes are not UTF-8, which would itself be a defect.
    #[must_use]
    pub fn text(&self) -> String {
        let guard = self
            .buffer
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8(guard.clone()).expect("log output is UTF-8")
    }

    /// The captured lines that came from the access log.
    #[must_use]
    pub fn access_lines(&self) -> Vec<String> {
        self.text()
            .lines()
            .filter(|line| line.contains(crate::access::TARGET))
            .map(str::to_owned)
            .collect()
    }
}

/// A loopback `vibe-talk-v1` bridge, for tests: the bridge a deployment puts behind
/// `conversation.websocket_url`, reduced to the part of the protocol `#331 resume-on-websocket`
/// is about. It starts a session on every connection, lists `replay` in that `session_started`
/// when it is built to take one, answers each `replay` frame with exactly one `replay_ack` —
/// accepted, or refused with a reason when it does not take a replay — and ends the session on
/// `quit`. Its frames are the contract's own types, so it speaks what the page validates.
///
/// It keeps every JSON frame each session was sent, in order, so a test can hold what a client
/// said to it: this server's hello says `quit` and nothing else, and a call that resumes says
/// `replay` before anything.
#[derive(Debug)]
pub struct FakeVoiceBridge {
    url: String,
    sessions: Arc<Mutex<Vec<Vec<serde_json::Value>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeVoiceBridge {
    /// Listen on a loopback port. `takes_replay` decides whether its sessions list `replay`.
    ///
    /// # Panics
    ///
    /// Panics if no loopback port can be bound, which leaves the test nothing to run against.
    pub async fn start(takes_replay: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port for the fake bridge");
        let url = format!(
            "ws://{}/voice",
            listener.local_addr().expect("the fake bridge's address")
        );
        let sessions = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&sessions);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve_fake_bridge_session(
                    stream,
                    takes_replay,
                    Arc::clone(&kept),
                ));
            }
        });
        Self {
            url,
            sessions,
            task,
        }
    }

    /// Where it listens, as `ws://127.0.0.1:PORT/voice`.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Every session so far, each as the JSON frames it was sent, in order.
    #[must_use]
    pub fn sessions(&self) -> Vec<Vec<serde_json::Value>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The `text` of every `replay` frame any session was sent, in order.
    #[must_use]
    pub fn replays(&self) -> Vec<String> {
        self.sessions()
            .iter()
            .flatten()
            .filter(|frame| frame["type"] == "replay")
            .filter_map(|frame| frame["text"].as_str().map(str::to_owned))
            .collect()
    }
}

impl Drop for FakeVoiceBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One session on the fake bridge: announce it, then answer `replay` and `quit` until it ends.
async fn serve_fake_bridge_session(
    stream: tokio::net::TcpStream,
    takes_replay: bool,
    sessions: Arc<Mutex<Vec<Vec<serde_json::Value>>>>,
) {
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;

    use crate::contract::{VibeTalkV1ClientFrame, VibeTalkV1ServerFrame};

    let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let index = {
        let mut all = sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        all.push(Vec::new());
        all.len() - 1
    };
    let frame = |frame: &VibeTalkV1ServerFrame| {
        Message::Text(serde_json::to_string(frame).unwrap_or_default().into())
    };
    let started = VibeTalkV1ServerFrame::SessionStarted {
        greeting: Some(false),
        session_id: Some(format!("fake-{}", index + 1)),
        capabilities: if takes_replay {
            vec![crate::voice_bridge::REPLAY.to_owned()]
        } else {
            Vec::new()
        },
    };
    if socket.send(frame(&started)).await.is_err() {
        return;
    }
    while let Some(Ok(message)) = socket.next().await {
        let raw = match message {
            Message::Text(raw) => raw,
            Message::Close(_) => break,
            _ => continue,
        };
        let value: serde_json::Value = serde_json::from_str(raw.as_ref()).unwrap_or_default();
        if let Some(kept) = sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(index)
        {
            kept.push(value.clone());
        }
        match serde_json::from_value::<VibeTalkV1ClientFrame>(value) {
            Ok(VibeTalkV1ClientFrame::Replay { .. }) => {
                let ack = if takes_replay {
                    VibeTalkV1ServerFrame::ReplayAck {
                        accepted: true,
                        reason: None,
                    }
                } else {
                    VibeTalkV1ServerFrame::ReplayAck {
                        accepted: false,
                        reason: Some("this bridge does not take a replay".to_owned()),
                    }
                };
                if socket.send(frame(&ack)).await.is_err() {
                    break;
                }
            }
            Ok(VibeTalkV1ClientFrame::Quit) => {
                let _ = socket.close(None).await;
                break;
            }
            _ => {}
        }
    }
}
