//! Server configuration: a TOML file, with environment overrides on top.
//!
//! Nothing here is host-specific and nothing here has a default that would let the server start
//! with a guessable credential. Every secret is required, is read from file or environment, and is
//! wrapped in [`Secret`] so it cannot be printed by accident.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;

use serde::Deserialize;

use crate::model::{ChannelId, ChannelInfo};

/// Environment variable naming the configuration file.
pub const ENV_CONFIG_PATH: &str = "GENT_TALK_CONFIG";
/// Environment variable overriding the bind address.
pub const ENV_BIND: &str = "GENT_TALK_BIND";
/// Environment variable carrying the Discord bot token.
pub const ENV_DISCORD_BOT_TOKEN: &str = "GENT_TALK_DISCORD_BOT_TOKEN";
/// Environment variable carrying the read-scope API token.
pub const ENV_READ_TOKEN: &str = "GENT_TALK_READ_TOKEN";
/// Environment variable carrying the write-scope API token.
pub const ENV_WRITE_TOKEN: &str = "GENT_TALK_WRITE_TOKEN";
/// Environment variable carrying the ElevenLabs API key.
pub const ENV_ELEVENLABS_API_KEY: &str = "GENT_TALK_ELEVENLABS_API_KEY";
/// Environment variable carrying the ElevenLabs agent id.
pub const ENV_ELEVENLABS_AGENT_ID: &str = "GENT_TALK_ELEVENLABS_AGENT_ID";
/// Environment variable overriding the channel list, as `id:label:rw` entries separated by commas.
pub const ENV_CHANNELS: &str = "GENT_TALK_CHANNELS";
/// Environment variable overriding the externally reachable base URL.
pub const ENV_PUBLIC_BASE_URL: &str = "GENT_TALK_PUBLIC_BASE_URL";

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
    /// Discord access.
    pub discord: DiscordConfig,
    /// API credentials for callers (the voice agent, and the web app).
    pub auth: AuthConfig,
    /// Channels this server will read.
    pub channels: Vec<ChannelInfo>,
    /// ElevenLabs wiring, when configured.
    pub elevenlabs: ElevenLabsConfig,
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
}

/// API credentials this server requires of its callers.
#[derive(Debug)]
pub struct AuthConfig {
    /// Token permitting reads.
    pub read_token: Secret,
    /// Token permitting reads AND posting.
    pub write_token: Secret,
}

/// ElevenLabs wiring. Both fields are optional in v0 because no call is made yet.
#[derive(Debug, Default)]
pub struct ElevenLabsConfig {
    /// Agent id, recorded so the deployment is self-describing.
    pub agent_id: Option<String>,
    /// API key, for the management calls a later version will make.
    pub api_key: Option<Secret>,
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
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileServer {
    bind: Option<String>,
    public_base_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDiscord {
    bot_token: Option<Secret>,
    api_base: Option<String>,
    default_fetch_limit: Option<u16>,
    max_fetch_limit: Option<u16>,
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
}

/// Default bind address: every interface inside a container, port 8080.
const DEFAULT_BIND: &str = "0.0.0.0:8080";
/// Default Discord API base.
pub const DEFAULT_DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const DEFAULT_FETCH_LIMIT: u16 = 50;
const DEFAULT_MAX_FETCH_LIMIT: u16 = 100;

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
        if default_fetch_limit == 0 || max_fetch_limit == 0 {
            return Err(ConfigError::Invalid {
                field: "discord.max_fetch_limit".to_owned(),
                detail: "fetch limits must be at least 1".to_owned(),
            });
        }
        if default_fetch_limit > max_fetch_limit {
            return Err(ConfigError::Invalid {
                field: "discord.default_fetch_limit".to_owned(),
                detail: format!("{default_fetch_limit} exceeds max_fetch_limit {max_fetch_limit}"),
            });
        }

        Ok(Self {
            bind,
            public_base_url: get(ENV_PUBLIC_BASE_URL)
                .map(str::to_owned)
                .or(file.server.public_base_url),
            discord: DiscordConfig {
                bot_token,
                api_base: file
                    .discord
                    .api_base
                    .unwrap_or_else(|| DEFAULT_DISCORD_API_BASE.to_owned()),
                default_fetch_limit,
                max_fetch_limit,
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
            },
        })
    }
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
    fn unknown_configuration_keys_are_refused() {
        let text = format!("{FULL}\n[nonsense]\nkey = 1\n");
        let err = Config::from_toml_and_env(&text, &env(&[])).expect_err("must refuse");
        assert!(
            matches!(err, ConfigError::Parse(_)),
            "a typo'd section must fail loudly, not be ignored"
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
        let cfg = Config::from_toml_and_env(FULL, &env(&[])).expect("valid config");
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("file-discord-token"), "leak: {rendered}");
        assert!(
            !rendered.contains("write-token-that-is-long-enough"),
            "leak: {rendered}"
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
