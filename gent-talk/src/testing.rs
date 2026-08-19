//! Scaffolding shared by unit tests and the integration tests under `tests/`.
//!
//! This is test support, not product code: it builds a server whose Discord is
//! [`crate::discord::fake::FakeDiscord`]. The binary never calls it.

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};

use crate::agent_backend::NoAgentBackend;
use crate::config::Config;
use crate::discord::fake::FakeDiscord;
use crate::elevenlabs::fake::{FakeElevenLabs, KNOWN_AGENT_ID, VALID_API_KEY};
use crate::retrieval::LexicalRanker;
use crate::state::AppState;

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
    let fake = Arc::new(FakeDiscord::new());
    fake.register_channel(&crate::model::ChannelId(READ_CHANNEL.to_owned()));
    fake.register_channel(&crate::model::ChannelId(WRITE_CHANNEL.to_owned()));
    let elevenlabs = Arc::new(FakeElevenLabs::new());
    let config = Config::from_toml_and_env(text, &BTreeMap::new()).expect("test config is valid");
    let state = AppState {
        config: Arc::new(config),
        discord: fake.clone(),
        ranker: Arc::new(LexicalRanker),
        agent: Arc::new(NoAgentBackend),
        elevenlabs: elevenlabs.clone(),
    };
    (state, fake, elevenlabs)
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
