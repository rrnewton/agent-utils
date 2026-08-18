//! Scaffolding shared by unit tests and the integration tests under `tests/`.
//!
//! This is test support, not product code: it builds a server whose Discord is
//! [`crate::discord::fake::FakeDiscord`]. The binary never calls it.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::agent_backend::NoAgentBackend;
use crate::config::Config;
use crate::discord::fake::FakeDiscord;
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
"#
    )
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
#[must_use]
pub fn state() -> (AppState, Arc<FakeDiscord>) {
    let fake = Arc::new(FakeDiscord::new());
    let state = AppState {
        config: Arc::new(config()),
        discord: fake.clone(),
        ranker: Arc::new(LexicalRanker),
        agent: Arc::new(NoAgentBackend),
    };
    (state, fake)
}
