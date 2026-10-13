//! Scaffolding shared by unit tests and the integration tests under `tests/`.
//!
//! This is test support, not product code: it builds a server whose Discord is
//! [`crate::discord::fake::FakeDiscord`]. The binary never calls it.

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
        headings: Arc::new(crate::heading::HeadingTags::new()),
        speech_tickets: Arc::new(crate::speech_tickets::SpeechTickets::new()),
        read_ahead: Arc::new(crate::read_ahead::ReadAhead::new()),
        recent_view_shadow: Arc::new(crate::recent_view::Shadow::new()),
        agent_override: Arc::new(crate::voice_prompt::OverrideAnswers::new()),
        post_gate: Arc::new(crate::post_gate::PostGate::new()),
        post_lanes: Arc::new(crate::post_lanes::PostLanes::new()),
        spoken_names: Arc::new(crate::speakable::SharedNames::new()),
        live: Arc::new(crate::live::LiveHub::new()),
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
/// `#210 voice-catchup-speed`. A fresh hub, so call it before anything is published.
#[must_use]
pub fn with_recent_timing(mut state: AppState, timing: crate::recent::Timing) -> AppState {
    state.live = Arc::new(crate::live::LiveHub::with_recent(Arc::new(
        crate::recent::RecentWindows::with_timing(timing),
    )));
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
