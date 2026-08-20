//! Server configuration: a TOML file, with environment overrides on top.
//!
//! Nothing here is host-specific and nothing here has a default that would let the server start
//! with a guessable credential. Every secret is required, is read from file or environment, and is
//! wrapped in [`Secret`] so it cannot be printed by accident.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::model::{ChannelId, ChannelInfo};
use crate::store::Retention;

/// Environment variable naming the configuration file.
pub const ENV_CONFIG_PATH: &str = "GENT_TALK_CONFIG";
/// Environment variable overriding the bind address.
pub const ENV_BIND: &str = "GENT_TALK_BIND";
/// Environment variable carrying the Discord bot token.
pub const ENV_DISCORD_BOT_TOKEN: &str = "GENT_TALK_DISCORD_BOT_TOKEN";
/// Environment variable overriding how often each channel is polled for inbound messages.
///
/// `0` turns live ingestion off, which is the default. See [`crate::live`].
pub const ENV_LIVE_POLL_SECONDS: &str = "GENT_TALK_LIVE_POLL_SECONDS";
/// Environment variable carrying the read-scope API token.
pub const ENV_READ_TOKEN: &str = "GENT_TALK_READ_TOKEN";
/// Environment variable carrying the write-scope API token.
pub const ENV_WRITE_TOKEN: &str = "GENT_TALK_WRITE_TOKEN";
/// Environment variable carrying the ElevenLabs API key.
pub const ENV_ELEVENLABS_API_KEY: &str = "GENT_TALK_ELEVENLABS_API_KEY";
/// Environment variable carrying the ElevenLabs agent id.
pub const ENV_ELEVENLABS_AGENT_ID: &str = "GENT_TALK_ELEVENLABS_AGENT_ID";
/// Environment variable overriding the ElevenLabs API base, so a test or a proxy can point
/// elsewhere.
pub const ENV_ELEVENLABS_API_BASE: &str = "GENT_TALK_ELEVENLABS_API_BASE";
/// Environment variable overriding the channel list, as `id:label:rw` entries separated by commas.
pub const ENV_CHANNELS: &str = "GENT_TALK_CHANNELS";
/// Environment variable overriding the externally reachable base URL.
pub const ENV_PUBLIC_BASE_URL: &str = "GENT_TALK_PUBLIC_BASE_URL";
/// Environment variable overriding the operator's time zone, as an IANA name.
pub const ENV_TIMEZONE: &str = "GENT_TALK_TIMEZONE";
/// Environment variable naming the SQLite file that holds durable state.
///
/// Unset means this server keeps nothing between restarts, and says so rather than pretending.
/// See [`crate::store`].
pub const ENV_STORAGE_PATH: &str = "GENT_TALK_STORAGE_PATH";

/// Environment variable turning conversation replay on or off (`1`/`true`/`yes`/`on`).
pub const ENV_REPLAY_ENABLED: &str = "GENT_TALK_REPLAY_ENABLED";
/// Environment variable capping the replayed transcript, in characters.
pub const ENV_REPLAY_MAX_CHARS: &str = "GENT_TALK_REPLAY_MAX_CHARS";
/// Environment variable capping the replayed transcript, in turns.
pub const ENV_REPLAY_MAX_TURNS: &str = "GENT_TALK_REPLAY_MAX_TURNS";
/// Environment variable choosing how the replay reaches the vendor.
pub const ENV_REPLAY_TRANSPORT: &str = "GENT_TALK_REPLAY_TRANSPORT";

/// Environment variable overriding the model a summariser is asked for.
pub const ENV_SUMMARY_MODEL: &str = "GENT_TALK_SUMMARY_MODEL";

/// Shortest API token this server will accept.
///
/// These tokens are the ONLY thing between the public internet and a bot that can post to the
/// owner's channels, so a short one is refused at startup rather than warned about.
pub const MIN_TOKEN_LEN: usize = 24;

/// A string that must never reach a log line.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wrap a secret value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the secret. Call sites should be few and obvious.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Constant-time-ish comparison against a presented credential.
    ///
    /// This compares every byte rather than returning early, so the reply latency does not leak
    /// how much of a guessed token was correct.
    #[must_use]
    pub fn matches(&self, presented: &str) -> bool {
        let expected = self.0.as_bytes();
        let got = presented.as_bytes();
        let mut diff = u8::from(expected.len() != got.len());
        for i in 0..expected.len().max(got.len()) {
            let a = expected.get(i).copied().unwrap_or(0);
            let b = got.get(i).copied().unwrap_or(0);
            diff |= a ^ b;
        }
        diff == 0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Everything the server needs to run.
#[derive(Debug)]
pub struct Config {
    /// Address to bind the HTTP listener to.
    pub bind: SocketAddr,
    /// Externally reachable base URL, when the deployment knows it.
    pub public_base_url: Option<String>,
    /// The zone every message timestamp is rendered into before anything speaks it.
    ///
    /// Configured, never hard-coded, and validated at load: a name the bundled database does not
    /// know stops the server rather than quietly becoming UTC. See [`crate::clock`].
    pub timezone: crate::clock::Zone,
    /// Discord access.
    pub discord: DiscordConfig,
    /// Whether and how an earlier transcript is replayed into a new call.
    pub replay: ReplayConfig,
    /// API credentials for callers (the voice agent, and the web app).
    pub auth: AuthConfig,
    /// Channels this server will read.
    pub channels: Vec<ChannelInfo>,
    /// ElevenLabs wiring, when configured.
    pub elevenlabs: ElevenLabsConfig,
    /// Durable state: where it lives, and how much of it is kept.
    pub storage: StorageConfig,
    /// Summarisation policy. Every field is part of the cache key; see
    /// [`crate::summarize::policy_version`].
    pub summaries: SummariesConfig,
}

/// What a summary is, in numbers.
///
/// **Every field here changes what a summary says**, so every field is folded into
/// [`crate::summarize::policy_version`] and therefore into the cache key. A field added later
/// without a line in that function is a field the cache cannot see, and the unit test over it is
/// what catches that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummariesConfig {
    /// Below this many characters a message is not summarised at all — it is already short
    /// enough to read, and summarising it would spend a model call to produce something longer
    /// than the original.
    pub threshold_chars: usize,
    /// How long a summary should be.
    pub target_chars: usize,
    /// How many preceding messages ride along as context.
    pub context_messages: usize,
    /// The model to ask, when a model backend is configured. `None` means the extractive default.
    pub model: Option<String>,
}

impl Default for SummariesConfig {
    fn default() -> Self {
        Self {
            threshold_chars: DEFAULT_SUMMARY_THRESHOLD_CHARS,
            target_chars: crate::summary::DEFAULT_SUMMARY_CHARS,
            context_messages: DEFAULT_SUMMARY_CONTEXT_MESSAGES,
            model: None,
        }
    }
}

/// Below this, a message is left alone.
pub const DEFAULT_SUMMARY_THRESHOLD_CHARS: usize = 400;
/// How many preceding messages ride along as context by default.
///
/// Three, and the honest note is that there is no offline oracle for this number: it was chosen,
/// not measured. It is configuration precisely so a deployment can disagree.
pub const DEFAULT_SUMMARY_CONTEXT_MESSAGES: usize = 3;

/// Where durable state lives, and how much of it is kept.
///
/// See [`crate::store`] for what is stored and why the absent case is a refusal rather than an
/// in-memory fallback.
#[derive(Debug, Default)]
pub struct StorageConfig {
    /// Absolute path to the SQLite file. `None` means this server keeps nothing.
    pub path: Option<PathBuf>,
    /// Bounds on what is kept.
    pub retention: Retention,
}

/// Default ceiling on a replayed transcript, in characters.
///
/// A guess, and labelled as one. It is billed per call and the model's window is finite; nothing
/// in this crate can measure whether it is right. `scripts/smoke-agent.py --replay-check` can.
pub const DEFAULT_REPLAY_MAX_CHARS: usize = 6000;
/// Default ceiling on a replayed transcript, in turns.
pub const DEFAULT_REPLAY_MAX_TURNS: usize = 40;

/// Reconstructing continuity across a hang-up. See [`crate::replay`].
#[derive(Debug)]
pub struct ReplayConfig {
    /// Whether the page may ask for a replay at all.
    ///
    /// **Off by default, and the reason is privacy rather than caution.** Every new call re-sends
    /// earlier conversation content — including Discord text written by other people — to the
    /// voice vendor. That is a decision an operator makes deliberately.
    pub enabled: bool,
    /// The budget.
    pub policy: crate::replay::ReplayPolicy,
    /// How the payload reaches the vendor.
    pub transport: crate::replay::Transport,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            policy: crate::replay::ReplayPolicy {
                max_chars: DEFAULT_REPLAY_MAX_CHARS,
                max_turns: DEFAULT_REPLAY_MAX_TURNS,
            },
            transport: crate::replay::Transport::ContextualUpdate,
        }
    }
}

/// Discord access parameters.
#[derive(Debug)]
pub struct DiscordConfig {
    /// Bot token. Sent as `Authorization: Bot <token>`.
    pub bot_token: Secret,
    /// API base, so a test or a proxy can point elsewhere.
    pub api_base: String,
    /// Default number of messages a fetch returns.
    pub default_fetch_limit: u16,
    /// Hard ceiling on a caller-requested fetch size.
    pub max_fetch_limit: u16,
    /// How many messages a count may walk before it gives up and answers "at least this many".
    ///
    /// A cost ceiling, not a preference. Discord has no message-count endpoint, so counting means
    /// paginating at a hundred messages per request against a shared rate limit; without a cap, a
    /// single spoken "how many messages are in there?" could fire dozens of requests at a busy
    /// channel and stall every other read behind it.
    pub max_count_scan: u32,
    /// How often each configured channel is polled for messages nobody asked for. `0` is OFF.
    ///
    /// **Off by default.** Every tick is one Discord request per channel against a rate limit this
    /// server does not handle — no `Retry-After`, and a 429 surfaces as HTTP 502 — so turning it
    /// on is a decision an operator makes with that in front of them, not a thing that happens
    /// because they upgraded. Refused below [`crate::live::MIN_POLL_SECONDS`].
    pub live_poll_seconds: u64,
}

/// API credentials this server requires of its callers.
#[derive(Debug)]
pub struct AuthConfig {
    /// Token permitting reads.
    pub read_token: Secret,
    /// Token permitting reads AND posting.
    pub write_token: Secret,
}

/// ElevenLabs wiring.
///
/// Both credentials are optional: the Discord half of this server is useful on its own, and a
/// deployment that has not wired a voice agent yet should still start. What is NOT optional is
/// what happens when something asks for a signed conversation URL without them — that is a loud,
/// specific refusal naming the absent setting, never a silent fallback. See
/// [`crate::elevenlabs::credentials`].
#[derive(Debug)]
pub struct ElevenLabsConfig {
    /// Agent id, recorded so the deployment is self-describing. Public: it identifies a widget.
    pub agent_id: Option<String>,
    /// Account API key, used to mint short-lived signed conversation URLs. A secret.
    pub api_key: Option<Secret>,
    /// API base, so a test or a proxy can point elsewhere.
    pub api_base: String,
}

impl Default for ElevenLabsConfig {
    fn default() -> Self {
        Self {
            agent_id: None,
            api_key: None,
            api_base: DEFAULT_ELEVENLABS_API_BASE.to_owned(),
        }
    }
}

/// Configuration could not be assembled.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The TOML file did not parse.
    #[error("configuration file did not parse: {0}")]
    Parse(#[from] toml::de::Error),
    /// The configuration file could not be read.
    #[error("configuration file {path} could not be read: {source}")]
    Read {
        /// Path that was attempted.
        path: String,
        /// Underlying I/O failure.
        source: std::io::Error,
    },
    /// A required value was absent from both the file and the environment.
    #[error("{0} is required: set it in the configuration file or in the environment")]
    Missing(String),
    /// A value was present but unusable.
    #[error("{field} is invalid: {detail}")]
    Invalid {
        /// Field name as the operator would write it.
        field: String,
        /// Why it was rejected.
        detail: String,
    },
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    server: FileServer,
    #[serde(default)]
    discord: FileDiscord,
    #[serde(default)]
    auth: FileAuth,
    #[serde(default)]
    channels: Vec<FileChannel>,
    #[serde(default)]
    elevenlabs: FileElevenLabs,
    #[serde(default)]
    storage: FileStorage,
    #[serde(default)]
    summaries: FileSummaries,
    #[serde(default)]
    replay: FileReplay,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileReplay {
    enabled: Option<bool>,
    max_chars: Option<usize>,
    max_turns: Option<usize>,
    transport: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileServer {
    bind: Option<String>,
    public_base_url: Option<String>,
    timezone: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDiscord {
    bot_token: Option<Secret>,
    api_base: Option<String>,
    default_fetch_limit: Option<u16>,
    max_fetch_limit: Option<u16>,
    max_count_scan: Option<u32>,
    live_poll_seconds: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAuth {
    read_token: Option<Secret>,
    write_token: Option<Secret>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileChannel {
    id: String,
    label: String,
    #[serde(default)]
    writable: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileElevenLabs {
    agent_id: Option<String>,
    api_key: Option<Secret>,
    api_base: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSummaries {
    threshold_chars: Option<usize>,
    target_chars: Option<usize>,
    context_messages: Option<usize>,
    model: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileStorage {
    path: Option<String>,
    max_conversations: Option<u16>,
    max_turns_per_conversation: Option<u16>,
    max_summaries: Option<u32>,
    max_dismissals: Option<u32>,
    retain_days: Option<u16>,
}

/// Default bind address: every interface inside a container, port 8080.
const DEFAULT_BIND: &str = "0.0.0.0:8080";
/// Default Discord API base.
pub const DEFAULT_DISCORD_API_BASE: &str = "https://discord.com/api/v10";
/// Default ElevenLabs API base.
pub const DEFAULT_ELEVENLABS_API_BASE: &str = "https://api.elevenlabs.io/v1";
/// Default operator time zone.
///
/// UTC because it is the one zone that is right for nobody in particular and therefore cannot be
/// mistaken for a considered choice — an operator who leaves it alone hears the same instant
/// Discord reported, correctly labelled, rather than a plausible-looking local time from whichever
/// datacentre the container happens to run in.
pub const DEFAULT_TIMEZONE: &str = "UTC";
const DEFAULT_FETCH_LIMIT: u16 = 50;
const DEFAULT_MAX_FETCH_LIMIT: u16 = 100;
/// Default cost ceiling for a count: five Discord requests' worth.
const DEFAULT_MAX_COUNT_SCAN: u32 = 500;

impl Config {
    /// Read the configuration file at `path`, then apply process-environment overrides.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the file cannot be read or parsed, or when the assembled
    /// configuration is incomplete or invalid.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        let env: BTreeMap<String, String> = std::env::vars().collect();
        Self::from_toml_and_env(&text, &env)
    }

    /// Assemble a configuration from TOML text plus an environment map.
    ///
    /// Environment values take precedence over file values, so a container can be given its
    /// secrets without ever writing them to disk.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the text does not parse, a required value is absent, or a
    /// present value is unusable.
    pub fn from_toml_and_env(
        text: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        let file: FileConfig = toml::from_str(text)?;
        let get = |key: &str| env.get(key).map(String::as_str).filter(|v| !v.is_empty());

        let bind_text = get(ENV_BIND)
            .map(str::to_owned)
            .or(file.server.bind)
            .unwrap_or_else(|| DEFAULT_BIND.to_owned());
        let bind = bind_text
            .parse::<SocketAddr>()
            .map_err(|e| ConfigError::Invalid {
                field: "server.bind".to_owned(),
                detail: format!("{bind_text:?} is not an address:port ({e})"),
            })?;

        let timezone_name = get(ENV_TIMEZONE)
            .map(str::to_owned)
            .or(file.server.timezone)
            .unwrap_or_else(|| DEFAULT_TIMEZONE.to_owned());
        let timezone =
            crate::clock::zone(&timezone_name).map_err(|detail| ConfigError::Invalid {
                field: "server.timezone".to_owned(),
                detail,
            })?;

        let bot_token = get(ENV_DISCORD_BOT_TOKEN)
            .map(Secret::new)
            .or(file.discord.bot_token)
            .ok_or_else(|| ConfigError::Missing("discord.bot_token".to_owned()))?;
        if bot_token.expose().trim().is_empty() {
            return Err(ConfigError::Invalid {
                field: "discord.bot_token".to_owned(),
                detail: "is blank".to_owned(),
            });
        }

        let read_token = get(ENV_READ_TOKEN)
            .map(Secret::new)
            .or(file.auth.read_token)
            .ok_or_else(|| ConfigError::Missing("auth.read_token".to_owned()))?;
        let write_token = get(ENV_WRITE_TOKEN)
            .map(Secret::new)
            .or(file.auth.write_token)
            .ok_or_else(|| ConfigError::Missing("auth.write_token".to_owned()))?;
        check_token_strength("auth.read_token", &read_token)?;
        check_token_strength("auth.write_token", &write_token)?;
        if read_token == write_token {
            return Err(ConfigError::Invalid {
                field: "auth.write_token".to_owned(),
                detail: "must differ from auth.read_token, or the read scope grants posting"
                    .to_owned(),
            });
        }

        let channels = match get(ENV_CHANNELS) {
            Some(spec) => parse_channel_spec(spec)?,
            None => file
                .channels
                .into_iter()
                .map(|c| ChannelInfo {
                    id: ChannelId(c.id),
                    label: c.label,
                    writable: c.writable,
                })
                .collect(),
        };
        validate_channels(&channels)?;

        let default_fetch_limit = file
            .discord
            .default_fetch_limit
            .unwrap_or(DEFAULT_FETCH_LIMIT);
        let max_fetch_limit = file
            .discord
            .max_fetch_limit
            .unwrap_or(DEFAULT_MAX_FETCH_LIMIT);
        let max_count_scan = file
            .discord
            .max_count_scan
            .unwrap_or(DEFAULT_MAX_COUNT_SCAN);
        if default_fetch_limit == 0 || max_fetch_limit == 0 {
            return Err(ConfigError::Invalid {
                field: "discord.max_fetch_limit".to_owned(),
                detail: "fetch limits must be at least 1".to_owned(),
            });
        }
        if max_count_scan == 0 {
            return Err(ConfigError::Invalid {
                field: "discord.max_count_scan".to_owned(),
                detail: "must be at least 1, or counting can never answer anything".to_owned(),
            });
        }
        let live_poll_seconds = match get(ENV_LIVE_POLL_SECONDS) {
            Some(raw) => raw
                .trim()
                .parse::<u64>()
                .map_err(|e| ConfigError::Invalid {
                    field: "discord.live_poll_seconds".to_owned(),
                    detail: format!("{raw:?} is not a whole number of seconds ({e})"),
                })?,
            None => file.discord.live_poll_seconds.unwrap_or(0),
        };
        if live_poll_seconds != 0 && live_poll_seconds < crate::live::MIN_POLL_SECONDS {
            return Err(ConfigError::Invalid {
                field: "discord.live_poll_seconds".to_owned(),
                detail: format!(
                    "{live_poll_seconds} is below the {} second floor. Every tick is one Discord \
                     request per channel against a rate limit this server does not handle; use 0 \
                     to turn live ingestion off.",
                    crate::live::MIN_POLL_SECONDS
                ),
            });
        }
        if default_fetch_limit > max_fetch_limit {
            return Err(ConfigError::Invalid {
                field: "discord.default_fetch_limit".to_owned(),
                detail: format!("{default_fetch_limit} exceeds max_fetch_limit {max_fetch_limit}"),
            });
        }

        let storage = storage_config(get(ENV_STORAGE_PATH), file.storage)?;
        let summaries = summaries_config(get(ENV_SUMMARY_MODEL), file.summaries)?;
        let replay = replay_config(
            [
                get(ENV_REPLAY_ENABLED),
                get(ENV_REPLAY_MAX_CHARS),
                get(ENV_REPLAY_MAX_TURNS),
                get(ENV_REPLAY_TRANSPORT),
            ],
            file.replay,
        )?;

        Ok(Self {
            bind,
            public_base_url: get(ENV_PUBLIC_BASE_URL)
                .map(str::to_owned)
                .or(file.server.public_base_url),
            timezone,
            discord: DiscordConfig {
                bot_token,
                api_base: file
                    .discord
                    .api_base
                    .unwrap_or_else(|| DEFAULT_DISCORD_API_BASE.to_owned()),
                default_fetch_limit,
                max_fetch_limit,
                max_count_scan,
                live_poll_seconds,
            },
            auth: AuthConfig {
                read_token,
                write_token,
            },
            channels,
            elevenlabs: ElevenLabsConfig {
                agent_id: get(ENV_ELEVENLABS_AGENT_ID)
                    .map(str::to_owned)
                    .or(file.elevenlabs.agent_id),
                api_key: get(ENV_ELEVENLABS_API_KEY)
                    .map(Secret::new)
                    .or(file.elevenlabs.api_key),
                api_base: get(ENV_ELEVENLABS_API_BASE)
                    .map(str::to_owned)
                    .or(file.elevenlabs.api_base)
                    .unwrap_or_else(|| DEFAULT_ELEVENLABS_API_BASE.to_owned()),
            },
            storage,
            summaries,
            replay,
        })
    }
}

/// Resolve the `[replay]` section.
fn replay_config(
    from_env: [Option<&str>; 4],
    file: FileReplay,
) -> Result<ReplayConfig, ConfigError> {
    let [enabled_env, chars_env, turns_env, transport_env] = from_env;
    let defaults = ReplayConfig::default();
    let number =
        |raw: Option<&str>, field: &'static str, from_file: Option<usize>, fallback| match raw {
            Some(text) => text
                .trim()
                .parse::<usize>()
                .map_err(|e| ConfigError::Invalid {
                    field: field.to_owned(),
                    detail: format!("{text:?} is not a whole number ({e})"),
                }),
            None => Ok(from_file.unwrap_or(fallback)),
        };
    let enabled = match enabled_env {
        Some(raw) => matches!(raw.trim(), "1" | "true" | "yes" | "on"),
        None => file.enabled.unwrap_or(defaults.enabled),
    };
    let max_chars = number(
        chars_env,
        "replay.max_chars",
        file.max_chars,
        defaults.policy.max_chars,
    )?;
    let max_turns = number(
        turns_env,
        "replay.max_turns",
        file.max_turns,
        defaults.policy.max_turns,
    )?;
    if max_chars == 0 || max_turns == 0 {
        // Zero is not "unlimited" and must not be readable as it. A budget of nothing produces an
        // empty replay on every call, which the page then correctly reports as "the agent starts
        // fresh" — a feature that is silently off while the setting says it is on.
        return Err(ConfigError::Invalid {
            field: if max_chars == 0 {
                "replay.max_chars".to_owned()
            } else {
                "replay.max_turns".to_owned()
            },
            detail: "must be at least 1. Zero is not 'no limit'; it is a replay that is always \
                     empty while the interface says resuming is on. Use replay.enabled = false."
                .to_owned(),
        });
    }
    let transport_text = transport_env.map(str::to_owned).or(file.transport);
    let transport = match transport_text {
        Some(text) => {
            crate::replay::Transport::parse(&text).ok_or_else(|| ConfigError::Invalid {
                field: "replay.transport".to_owned(),
                detail: format!(
                    "{text:?} is not a transport. Use \"contextual_update\" (the default) or \
                     \"client_data\"."
                ),
            })?
        }
        None => defaults.transport,
    };
    Ok(ReplayConfig {
        enabled,
        policy: crate::replay::ReplayPolicy {
            max_chars,
            max_turns,
        },
        transport,
    })
}

/// Resolve the `[summaries]` section.
fn summaries_config(
    model_from_env: Option<&str>,
    file: FileSummaries,
) -> Result<SummariesConfig, ConfigError> {
    let defaults = SummariesConfig::default();
    let summaries = SummariesConfig {
        threshold_chars: file.threshold_chars.unwrap_or(defaults.threshold_chars),
        target_chars: file.target_chars.unwrap_or(defaults.target_chars),
        context_messages: file.context_messages.unwrap_or(defaults.context_messages),
        model: model_from_env
            .map(str::to_owned)
            .or(file.model)
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty()),
    };
    if summaries.target_chars == 0 {
        return Err(ConfigError::Invalid {
            field: "summaries.target_chars".to_owned(),
            detail: "must be at least 1, or every summary is the empty string".to_owned(),
        });
    }
    if summaries.threshold_chars < summaries.target_chars {
        // Otherwise the summary of a message just over the threshold is LONGER than the message,
        // which is a cost with a negative benefit and reads on screen as the page padding things
        // out.
        return Err(ConfigError::Invalid {
            field: "summaries.threshold_chars".to_owned(),
            detail: format!(
                "{} is below summaries.target_chars {}, so a summarised message would come back \
                 longer than it started",
                summaries.threshold_chars, summaries.target_chars
            ),
        });
    }
    Ok(summaries)
}

/// Resolve the `[storage]` section.
///
/// The one rule worth spelling out is that the path must be **absolute**. A relative path is
/// resolved against the process's working directory, which inside a container is a directory in
/// the image — so a store configured that way works perfectly until the image is rebuilt and then
/// silently loses everything. That is the exact failure this setting exists to prevent, so it is
/// refused at load rather than warned about at runtime.
fn storage_config(from_env: Option<&str>, file: FileStorage) -> Result<StorageConfig, ConfigError> {
    let path = from_env
        .map(str::to_owned)
        .or(file.path)
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    if let Some(path) = &path {
        if !path.is_absolute() {
            return Err(ConfigError::Invalid {
                field: "storage.path".to_owned(),
                detail: format!(
                    "{} is relative; give an absolute path on a mounted volume, or a rebuild of \
                     the image silently discards everything stored so far",
                    path.display()
                ),
            });
        }
    }

    let retention = Retention {
        max_conversations: file
            .max_conversations
            .unwrap_or(crate::store::DEFAULT_MAX_CONVERSATIONS),
        max_turns_per_conversation: file
            .max_turns_per_conversation
            .unwrap_or(crate::store::DEFAULT_MAX_TURNS_PER_CONVERSATION),
        max_summaries: file
            .max_summaries
            .unwrap_or(crate::store::DEFAULT_MAX_SUMMARIES),
        max_dismissals: file
            .max_dismissals
            .unwrap_or(crate::store::DEFAULT_MAX_DISMISSALS),
        retain_days: file
            .retain_days
            .unwrap_or(crate::store::DEFAULT_RETAIN_DAYS),
    };
    if retention.max_conversations == 0 {
        return Err(ConfigError::Invalid {
            field: "storage.max_conversations".to_owned(),
            detail: "must be at least 1; zero would delete every conversation as it was written"
                .to_owned(),
        });
    }
    if retention.max_turns_per_conversation == 0 {
        return Err(ConfigError::Invalid {
            field: "storage.max_turns_per_conversation".to_owned(),
            detail: "must be at least 1, or no turn can ever be recorded".to_owned(),
        });
    }
    if retention.max_summaries == 0 {
        return Err(ConfigError::Invalid {
            field: "storage.max_summaries".to_owned(),
            detail: "must be at least 1; zero would delete every cached summary as it was written"
                .to_owned(),
        });
    }
    if retention.max_dismissals == 0 {
        return Err(ConfigError::Invalid {
            field: "storage.max_dismissals".to_owned(),
            detail: "must be at least 1; zero would put every message the owner dealt with back \
                     in front of him the moment he cleared it"
                .to_owned(),
        });
    }
    Ok(StorageConfig { path, retention })
}

fn check_token_strength(field: &str, token: &Secret) -> Result<(), ConfigError> {
    if token.expose().len() < MIN_TOKEN_LEN {
        return Err(ConfigError::Invalid {
            field: field.to_owned(),
            detail: format!(
                "must be at least {MIN_TOKEN_LEN} characters; this token guards a bot that can \
                 post to your channels"
            ),
        });
    }
    Ok(())
}

fn validate_channels(channels: &[ChannelInfo]) -> Result<(), ConfigError> {
    if channels.is_empty() {
        return Err(ConfigError::Missing("channels".to_owned()));
    }
    let mut seen = std::collections::BTreeSet::new();
    for channel in channels {
        if channel.id.as_str().is_empty() {
            return Err(ConfigError::Invalid {
                field: "channels.id".to_owned(),
                detail: "is blank".to_owned(),
            });
        }
        if !seen.insert(channel.id.clone()) {
            return Err(ConfigError::Invalid {
                field: "channels.id".to_owned(),
                detail: format!("{} is listed twice", channel.id),
            });
        }
    }
    Ok(())
}

/// Parse the `GENT_TALK_CHANNELS` form: `id:label:rw,id:label` (missing mode means read-only).
fn parse_channel_spec(spec: &str) -> Result<Vec<ChannelInfo>, ConfigError> {
    let mut out = Vec::new();
    for entry in spec.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let mut parts = entry.split(':');
        let id = parts.next().unwrap_or_default().trim();
        let label = parts.next().unwrap_or_default().trim();
        let mode = parts.next().unwrap_or("ro").trim();
        if parts.next().is_some() {
            return Err(ConfigError::Invalid {
                field: ENV_CHANNELS.to_owned(),
                detail: format!("{entry:?} has more than three colon-separated fields"),
            });
        }
        if id.is_empty() || label.is_empty() {
            return Err(ConfigError::Invalid {
                field: ENV_CHANNELS.to_owned(),
                detail: format!("{entry:?} must be id:label[:rw|:ro]"),
            });
        }
        let writable = match mode {
            "rw" => true,
            "ro" => false,
            other => {
                return Err(ConfigError::Invalid {
                    field: ENV_CHANNELS.to_owned(),
                    detail: format!("{other:?} is not rw or ro"),
                })
            }
        };
        out.push(ChannelInfo {
            id: ChannelId(id.to_owned()),
            label: label.to_owned(),
            writable,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    const FULL: &str = r#"
[server]
bind = "127.0.0.1:9000"

[discord]
bot_token = "file-discord-token"

[auth]
read_token = "read-token-that-is-long-enough"
write_token = "write-token-that-is-long-enough"

[[channels]]
id = "111"
label = "lead"
writable = true
"#;

    #[test]
    fn file_only_configuration_loads() {
        let cfg = Config::from_toml_and_env(FULL, &env(&[])).expect("valid config");
        assert_eq!(cfg.bind.to_string(), "127.0.0.1:9000");
        assert_eq!(cfg.discord.bot_token.expose(), "file-discord-token");
        assert_eq!(cfg.channels.len(), 1);
        assert!(cfg.channels[0].writable);
        assert_eq!(cfg.discord.api_base, DEFAULT_DISCORD_API_BASE);
    }

    #[test]
    fn environment_overrides_the_file() {
        let cfg = Config::from_toml_and_env(
            FULL,
            &env(&[
                (ENV_BIND, "0.0.0.0:1234"),
                (ENV_DISCORD_BOT_TOKEN, "env-discord-token"),
            ]),
        )
        .expect("valid config");
        assert_eq!(cfg.bind.to_string(), "0.0.0.0:1234");
        assert_eq!(cfg.discord.bot_token.expose(), "env-discord-token");
    }

    #[test]
    fn empty_environment_value_does_not_override() {
        // An unset variable rendered as "" by a container runtime must not blank out the file.
        let cfg = Config::from_toml_and_env(FULL, &env(&[(ENV_DISCORD_BOT_TOKEN, "")]))
            .expect("valid config");
        assert_eq!(cfg.discord.bot_token.expose(), "file-discord-token");
    }

    #[test]
    fn missing_bot_token_is_refused() {
        let text = FULL.replace("bot_token = \"file-discord-token\"", "");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Missing(field) if field == "discord.bot_token"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn short_api_token_is_refused() {
        let text = FULL.replace("read-token-that-is-long-enough", "short");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "auth.read_token"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn identical_read_and_write_tokens_are_refused() {
        let text = FULL.replace(
            "write-token-that-is-long-enough",
            "read-token-that-is-long-enough",
        );
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "auth.write_token"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn no_channels_is_refused() {
        let text = FULL
            .split("[[channels]]")
            .next()
            .expect("prefix")
            .to_owned();
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Missing(field) if field == "channels"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn duplicate_channel_ids_are_refused() {
        let err = Config::from_toml_and_env(
            FULL,
            &env(&[(ENV_CHANNELS, "111:lead:rw,111:lead-again:ro")]),
        )
        .expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "channels.id"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn channel_spec_defaults_to_read_only() {
        let cfg =
            Config::from_toml_and_env(FULL, &env(&[(ENV_CHANNELS, "222:ops , 333:build:rw")]))
                .expect("valid config");
        assert_eq!(cfg.channels.len(), 2);
        assert_eq!(cfg.channels[0].label, "ops");
        assert!(
            !cfg.channels[0].writable,
            "a channel with no mode must default to read-only"
        );
        assert!(cfg.channels[1].writable);
    }

    #[test]
    fn channel_spec_rejects_an_unknown_mode() {
        let err = Config::from_toml_and_env(FULL, &env(&[(ENV_CHANNELS, "222:ops:writable")]))
            .expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == ENV_CHANNELS),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn fetch_limits_must_be_ordered() {
        let text = format!("{FULL}\ndefault_fetch_limit_placeholder = 0\n")
            .replace(
                "[discord]",
                "[discord]\ndefault_fetch_limit = 90\nmax_fetch_limit = 20",
            )
            .replace("default_fetch_limit_placeholder = 0\n", "");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "discord.default_fetch_limit"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn the_timezone_defaults_to_utc_and_can_be_set_from_file_or_environment() {
        let cfg = Config::from_toml_and_env(FULL, &env(&[])).expect("valid config");
        assert_eq!(cfg.timezone.name(), DEFAULT_TIMEZONE);

        let text = FULL.replace("[server]", "[server]\ntimezone = \"America/New_York\"");
        let cfg = Config::from_toml_and_env(&text, &env(&[])).expect("valid config");
        assert_eq!(cfg.timezone.name(), "America/New_York");

        let cfg = Config::from_toml_and_env(&text, &env(&[(ENV_TIMEZONE, "Europe/Berlin")]))
            .expect("valid config");
        assert_eq!(
            cfg.timezone.name(),
            "Europe/Berlin",
            "the environment must win, so a container can be told its operator's zone"
        );
    }

    #[test]
    fn an_unknown_timezone_stops_the_server_instead_of_becoming_utc() {
        // Silently defaulting is the failure this whole feature exists to prevent: the operator
        // would hear confidently-labelled times in the wrong zone and have no way to notice.
        let text = FULL.replace("[server]", "[server]\ntimezone = \"America/Nowhere\"");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, detail }
                if field == "server.timezone" && detail.contains("America/Nowhere")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn the_count_scan_ceiling_defaults_and_must_be_usable() {
        let cfg = Config::from_toml_and_env(FULL, &env(&[])).expect("valid config");
        assert_eq!(cfg.discord.max_count_scan, DEFAULT_MAX_COUNT_SCAN);

        let text = FULL.replace("[discord]", "[discord]\nmax_count_scan = 1200");
        let cfg = Config::from_toml_and_env(&text, &env(&[])).expect("valid config");
        assert_eq!(cfg.discord.max_count_scan, 1200);

        let text = FULL.replace("[discord]", "[discord]\nmax_count_scan = 0");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "discord.max_count_scan"),
            "a zero ceiling means every count answers \"at least zero\": {err}"
        );
    }

    #[test]
    fn live_ingestion_is_off_unless_asked_for_and_refuses_a_reckless_interval() {
        // OFF by default is the whole safety property here: upgrading this server must not start
        // a new, permanent stream of Discord requests on an operator's account without them
        // having typed anything.
        let cfg = Config::from_toml_and_env(FULL, &env(&[])).expect("valid config");
        assert_eq!(cfg.discord.live_poll_seconds, 0);

        let text = FULL.replace("[discord]", "[discord]\nlive_poll_seconds = 30");
        let cfg = Config::from_toml_and_env(&text, &env(&[])).expect("valid config");
        assert_eq!(cfg.discord.live_poll_seconds, 30);

        // The environment wins over the file, like every other setting here.
        let cfg = Config::from_toml_and_env(&text, &env(&[(ENV_LIVE_POLL_SECONDS, "60")]))
            .expect("valid config");
        assert_eq!(cfg.discord.live_poll_seconds, 60);

        // ...including turning it back OFF, which is the one an operator reaches for in a hurry.
        let cfg = Config::from_toml_and_env(&text, &env(&[(ENV_LIVE_POLL_SECONDS, "0")]))
            .expect("valid config");
        assert_eq!(cfg.discord.live_poll_seconds, 0);

        // Refused, not clamped. A silently-corrected 1 would look like it was honoured.
        let text = FULL.replace("[discord]", "[discord]\nlive_poll_seconds = 1");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, detail }
                if field == "discord.live_poll_seconds" && detail.contains("rate limit")),
            "unexpected error: {err}"
        );

        let err = Config::from_toml_and_env(FULL, &env(&[(ENV_LIVE_POLL_SECONDS, "soon")]))
            .expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. }
                if field == "discord.live_poll_seconds"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resuming_is_off_by_default_and_its_budget_cannot_be_configured_into_a_lie() {
        // OFF by default is the safety property, and it is about privacy: every new call re-sends
        // prior conversation content to a vendor, so an upgrade must not start doing that.
        let cfg = Config::from_toml_and_env(FULL, &env(&[])).expect("valid config");
        assert!(!cfg.replay.enabled);
        assert_eq!(cfg.replay.policy.max_chars, DEFAULT_REPLAY_MAX_CHARS);
        assert_eq!(cfg.replay.policy.max_turns, DEFAULT_REPLAY_MAX_TURNS);
        assert_eq!(
            cfg.replay.transport,
            crate::replay::Transport::ContextualUpdate,
            "the transport that does not depend on a dashboard setting is the default"
        );

        let text = format!("{FULL}\n[replay]\nenabled = true\nmax_turns = 5\n");
        let cfg = Config::from_toml_and_env(&text, &env(&[])).expect("valid config");
        assert!(cfg.replay.enabled);
        assert_eq!(cfg.replay.policy.max_turns, 5);

        let cfg = Config::from_toml_and_env(&text, &env(&[(ENV_REPLAY_ENABLED, "0")]))
            .expect("valid config");
        assert!(
            !cfg.replay.enabled,
            "the environment has to be able to turn it back OFF, which is the direction an \
             operator reaches for in a hurry"
        );

        // Zero is not "no limit". It is a replay that is always empty while the interface says
        // resuming is on, which is the silent-nothing this feature must not be able to become.
        let text = format!("{FULL}\n[replay]\nenabled = true\nmax_chars = 0\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, detail }
                if field == "replay.max_chars" && detail.contains("not 'no limit'")),
            "unexpected error: {err}"
        );

        // An unknown transport is refused rather than silently defaulted: a typo'd
        // "prompt_override" that quietly became contextual_update would be a deployment behaving
        // differently from its own configuration file.
        let text = format!("{FULL}\n[replay]\ntransport = \"prompt_override\"\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "replay.transport"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn the_shipped_example_configuration_is_one_this_server_would_actually_accept() {
        // Both example files are the first thing an operator copies, and `deny_unknown_fields`
        // means a section documented but never wired up is a server that refuses to start. Nothing
        // checked either of them until `#46 conversation-replay` added a section to both.
        let example = include_str!("../gent-talk.example.toml")
            .replace(
                "REPLACE-ME-at-least-24-characters",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .replace(
                "REPLACE-ME-different-and-at-least-24-characters",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            );
        Config::from_toml_and_env(&example, &env(&[])).expect("the shipped example must parse");
    }

    #[test]
    fn unknown_configuration_keys_are_refused() {
        let text = format!("{FULL}\n[nonsense]\nkey = 1\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(err, ConfigError::Parse(_)),
            "a typo'd section must fail loudly, not be ignored"
        );
    }

    #[test]
    fn the_elevenlabs_credentials_come_from_file_or_environment() {
        let text = format!(
            "{FULL}\n[elevenlabs]\nagent_id = \"agent_from_file\"\napi_key = \"key-from-file\"\n"
        );
        let cfg = Config::from_toml_and_env(&text, &env(&[])).expect("valid config");
        assert_eq!(cfg.elevenlabs.agent_id.as_deref(), Some("agent_from_file"));
        assert_eq!(
            cfg.elevenlabs.api_key.as_ref().map(Secret::expose),
            Some("key-from-file")
        );
        assert_eq!(cfg.elevenlabs.api_base, DEFAULT_ELEVENLABS_API_BASE);

        let cfg = Config::from_toml_and_env(
            &text,
            &env(&[
                (ENV_ELEVENLABS_AGENT_ID, "agent_from_env"),
                (ENV_ELEVENLABS_API_KEY, "key-from-env"),
                (ENV_ELEVENLABS_API_BASE, "https://proxy.test/v1"),
            ]),
        )
        .expect("valid config");
        assert_eq!(cfg.elevenlabs.agent_id.as_deref(), Some("agent_from_env"));
        assert_eq!(
            cfg.elevenlabs.api_key.as_ref().map(Secret::expose),
            Some("key-from-env")
        );
        assert_eq!(cfg.elevenlabs.api_base, "https://proxy.test/v1");
    }

    #[test]
    fn an_unconfigured_elevenlabs_section_still_loads() {
        // The Discord half of this server is useful on its own; a deployment with no voice agent
        // must still start, and fail only when something actually asks for a signed URL.
        let cfg = Config::from_toml_and_env(FULL, &env(&[])).expect("valid config");
        assert!(cfg.elevenlabs.agent_id.is_none());
        assert!(cfg.elevenlabs.api_key.is_none());
    }

    #[test]
    fn storage_is_off_unless_a_path_is_given() {
        let cfg = Config::from_toml_and_env(FULL, &env(&[])).expect("valid config");
        assert!(
            cfg.storage.path.is_none(),
            "a server with no storage configured must not invent a path for the owner's speech"
        );
        assert_eq!(
            cfg.storage.retention,
            crate::store::Retention::default(),
            "the bounds still have to be defined, so the absent case is one decision not two"
        );
    }

    #[test]
    fn the_storage_path_comes_from_file_or_environment() {
        let text = format!("{FULL}\n[storage]\npath = \"/var/lib/gent-talk/from-file.sqlite3\"\n");
        let cfg = Config::from_toml_and_env(&text, &env(&[])).expect("valid config");
        assert_eq!(
            cfg.storage.path.as_deref(),
            Some(Path::new("/var/lib/gent-talk/from-file.sqlite3"))
        );

        let cfg =
            Config::from_toml_and_env(&text, &env(&[(ENV_STORAGE_PATH, "/data/from-env.sqlite3")]))
                .expect("valid config");
        assert_eq!(
            cfg.storage.path.as_deref(),
            Some(Path::new("/data/from-env.sqlite3")),
            "the environment must win, so a container can be told where its volume is"
        );
    }

    #[test]
    fn a_relative_storage_path_is_refused() {
        // A relative path inside a container points into the IMAGE, so the store works until the
        // next rebuild and then is gone. Refusing is the whole point of the setting.
        let text = format!("{FULL}\n[storage]\npath = \"state/gent-talk.sqlite3\"\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "storage.path"),
            "unexpected error: {err}"
        );

        let err = Config::from_toml_and_env(FULL, &env(&[(ENV_STORAGE_PATH, "./state.sqlite3")]))
            .expect_err("must refuse the environment form too");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "storage.path"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn retention_bounds_are_configurable_and_must_be_usable() {
        let text = format!(
            "{FULL}\n[storage]\npath = \"/data/s.sqlite3\"\nmax_conversations = 3\n\
             max_turns_per_conversation = 7\nmax_summaries = 11\nretain_days = 0\n"
        );
        let cfg = Config::from_toml_and_env(&text, &env(&[])).expect("valid config");
        assert_eq!(cfg.storage.retention.max_conversations, 3);
        assert_eq!(cfg.storage.retention.max_turns_per_conversation, 7);
        assert_eq!(
            cfg.storage.retention.max_summaries, 11,
            "the summary cache is bounded too, and the bound has to be settable"
        );
        assert_eq!(
            cfg.storage.retention.retain_days, 0,
            "zero days must mean no age limit, not immediate deletion"
        );

        let text = format!("{FULL}\n[storage]\nmax_conversations = 0\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "storage.max_conversations"),
            "unexpected error: {err}"
        );

        let text = format!("{FULL}\n[storage]\nmax_turns_per_conversation = 0\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "storage.max_turns_per_conversation"),
            "unexpected error: {err}"
        );

        let text = format!("{FULL}\n[storage]\nmax_summaries = 0\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "storage.max_summaries"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn a_typo_in_the_storage_section_is_refused_rather_than_ignored() {
        let text = format!("{FULL}\n[storage]\npth = \"/data/s.sqlite3\"\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(err, ConfigError::Parse(_)),
            "a misspelled key must not silently leave storage off: {err}"
        );
    }

    #[test]
    fn the_summary_policy_defaults_and_is_configurable() {
        let cfg = Config::from_toml_and_env(FULL, &env(&[])).expect("valid config");
        assert_eq!(cfg.summaries, SummariesConfig::default());

        let text = format!(
            "{FULL}\n[summaries]\nthreshold_chars = 500\ntarget_chars = 80\n\
             context_messages = 1\nmodel = \"some-model\"\n"
        );
        let cfg = Config::from_toml_and_env(&text, &env(&[])).expect("valid config");
        assert_eq!(cfg.summaries.threshold_chars, 500);
        assert_eq!(cfg.summaries.target_chars, 80);
        assert_eq!(cfg.summaries.context_messages, 1);
        assert_eq!(cfg.summaries.model.as_deref(), Some("some-model"));

        let cfg = Config::from_toml_and_env(&text, &env(&[(ENV_SUMMARY_MODEL, "from-env")]))
            .expect("valid config");
        assert_eq!(cfg.summaries.model.as_deref(), Some("from-env"));
    }

    #[test]
    fn a_summary_longer_than_the_message_it_replaces_is_refused() {
        // Otherwise a message just over the threshold comes back LONGER than it started, which is
        // a cost with a negative benefit and reads on screen as the page padding things out.
        let text = format!("{FULL}\n[summaries]\nthreshold_chars = 50\ntarget_chars = 160\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "summaries.threshold_chars"),
            "unexpected error: {err}"
        );

        let text = format!("{FULL}\n[summaries]\ntarget_chars = 0\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(&err, ConfigError::Invalid { field, .. } if field == "summaries.target_chars"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn secret_debug_output_is_redacted() {
        let secret = Secret::new("hunter2-hunter2-hunter2");
        let rendered = format!("{secret:?}");
        assert!(
            !rendered.contains("hunter2"),
            "Secret leaked in Debug: {rendered}"
        );
        assert_eq!(rendered, "Secret(<redacted>)");
    }

    #[test]
    fn config_debug_output_does_not_leak_tokens() {
        let text = format!("{FULL}\n[elevenlabs]\napi_key = \"xi-key-in-the-file\"\n");
        let cfg = Config::from_toml_and_env(&text, &env(&[])).expect("valid config");
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("file-discord-token"), "leak: {rendered}");
        assert!(
            !rendered.contains("write-token-that-is-long-enough"),
            "leak: {rendered}"
        );
        assert!(
            !rendered.contains("xi-key-in-the-file"),
            "the ElevenLabs key leaked through Debug: {rendered}"
        );
    }

    #[test]
    fn secret_matches_only_the_exact_value() {
        let secret = Secret::new("abcdefgh");
        assert!(secret.matches("abcdefgh"));
        assert!(!secret.matches("abcdefg"));
        assert!(!secret.matches("abcdefghi"));
        assert!(!secret.matches("abcdefgH"));
        assert!(!secret.matches(""));
    }
}
