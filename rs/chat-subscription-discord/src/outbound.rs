//! The bridge's one-shot `outbound_command` helper for Discord.
//!
//! One request line in, one response line out, in the bridge's v1 outbound contract (the same
//! one the Slack helper speaks):
//!
//! ```json
//! {"version":1,"id":"<uuid>","action":"send","channel_id":"<chan>","thread_id":"<chan>/threads/<root>"|null,"text":"…"}
//! {"version":1,"id":"<uuid>","action":"ensure_reaction","channel_id":"<chan>","message_id":"<chan>/messages/<id>","emoji":"✅"}
//! ```
//!
//! A `send` with a thread posts a Discord reply to the thread's root message (this is how relay
//! receipts land "in the post's thread"); with `thread_id: null` it posts at channel level.
//! Text over Discord's 2,000-character limit is split at line breaks, except that a post
//! containing a fenced block (a relay envelope) is refused as `too_long` rather than split: a
//! later chunk would carry no fence opener. Configure the relay route's `max_post_bytes` at or
//! below 2000 for Discord.
//!
//! Every post sets `allowed_mentions.parse = []` and a nonce derived from the request id with
//! `enforce_nonce`, so a retried request does not post twice within Discord's nonce window.
//! Other actions (`update`, `delete`, `ensure_rejection`, `resolve_sender`, `list_members`)
//! are refused as `unsupported_action`, outcome `not_applied`: rejection reactions need a
//! human-author check this draft does not yet implement, so it never adds one unguarded.

use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::ids::{self, Snowflake};
use crate::markdown;
use crate::transport::{
    Applied, CurlTransport, DiscordRest, RestError, TokenSource, DEFAULT_API_BASE,
    MAX_CONTENT_CHARS,
};

/// Largest request accepted on standard input.
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;

/// How visible text is prepared before posting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextMode {
    /// Keep markdown, break mentions (default).
    Mentions,
    /// Render literally: escape markdown and break mentions.
    Literal,
    /// Post unchanged (mentions still cannot ping: `allowed_mentions` is empty).
    Raw,
}

/// Helper settings from its command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundConfig {
    /// Channels the helper may post in.
    pub channels: Vec<Snowflake>,
    /// Token source.
    pub token: TokenSource,
    /// Absolute curl path.
    pub curl_path: PathBuf,
    /// REST base URL.
    pub api_base: String,
    /// Per-call timeout.
    pub timeout_seconds: u32,
    /// Text preparation.
    pub text_mode: TextMode,
}

impl OutboundConfig {
    /// Parse `--channel=ID` (repeatable, required), `--token-file=PATH` [`--token-key=KEY`]
    /// or `--token-env=NAME`, `--curl=PATH`, `--api-base=URL`, `--timeout-seconds=N`,
    /// `--text=mentions|literal|raw`.
    ///
    /// # Errors
    ///
    /// An unknown or invalid argument, or no channel.
    pub fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut channels = Vec::new();
        let mut token_file = None;
        let mut token_key = "DISCORD_BOT_TOKEN".to_owned();
        let mut token_env = None;
        let mut curl_path = PathBuf::from("/usr/bin/curl");
        let mut api_base = DEFAULT_API_BASE.to_owned();
        let mut timeout_seconds = 20;
        let mut text_mode = TextMode::Mentions;
        for argument in arguments {
            let (key, value) = argument
                .split_once('=')
                .ok_or_else(|| format!("argument {argument} must be --key=value"))?;
            match key {
                "--channel" => channels
                    .push(Snowflake::parse(value).ok_or("--channel must be a Discord channel id")?),
                "--token-file" => token_file = Some(PathBuf::from(value)),
                "--token-key" => value.clone_into(&mut token_key),
                "--token-env" => token_env = Some(value.to_owned()),
                "--curl" => curl_path = PathBuf::from(value),
                "--api-base" => value.clone_into(&mut api_base),
                "--timeout-seconds" => {
                    timeout_seconds = value
                        .parse()
                        .ok()
                        .filter(|seconds| (1..=30).contains(seconds))
                        .ok_or("--timeout-seconds must be 1-30")?;
                }
                "--text" => {
                    text_mode = match value {
                        "mentions" => TextMode::Mentions,
                        "literal" => TextMode::Literal,
                        "raw" => TextMode::Raw,
                        _ => return Err("--text must be mentions, literal or raw".to_owned()),
                    }
                }
                _ => return Err(format!("unknown argument {key}")),
            }
        }
        if channels.is_empty() {
            return Err("at least one --channel is required".to_owned());
        }
        let token = match (token_file, token_env) {
            (Some(_), Some(_)) => {
                return Err("use --token-file or --token-env, not both".to_owned())
            }
            (Some(path), None) => TokenSource::DotEnv {
                path,
                key: token_key,
            },
            (None, Some(name)) => TokenSource::Env(name),
            (None, None) => TokenSource::Env("DISCORD_BOT_TOKEN".to_owned()),
        };
        Ok(Self {
            channels,
            token,
            curl_path,
            api_base,
            timeout_seconds,
            text_mode,
        })
    }

    /// Build the production transport.
    ///
    /// # Errors
    ///
    /// The token cannot be loaded or curl is misconfigured.
    pub fn transport(&self) -> Result<Box<dyn DiscordRest>, String> {
        let token = self.token.load()?;
        CurlTransport::new(
            self.curl_path.clone(),
            self.api_base.clone(),
            token,
            self.timeout_seconds,
        )
        .map(|transport| Box::new(transport) as Box<dyn DiscordRest>)
    }
}

struct Refusal {
    code: String,
    detail: String,
    applied: Applied,
    retryable: bool,
}

impl Refusal {
    fn not_applied(code: &str, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            detail: detail.into(),
            applied: Applied::NotApplied,
            retryable,
        }
    }
}

impl From<RestError> for Refusal {
    fn from(error: RestError) -> Self {
        Self {
            code: error.code,
            detail: error.detail,
            applied: error.applied,
            retryable: error.retryable,
        }
    }
}

fn line(value: &Value) -> Vec<u8> {
    let mut encoded = serde_json::to_vec(value).unwrap_or_default();
    encoded.push(b'\n');
    encoded
}

fn respond_error(id: &Value, action: &Value, refusal: &Refusal) -> Vec<u8> {
    line(&json!({
        "version": 1, "id": id, "action": action, "ok": false,
        "error": {
            "code": refusal.code,
            "detail": refusal.detail.chars().take(500).collect::<String>(),
            "outcome": match refusal.applied {
                Applied::NotApplied => "not_applied",
                Applied::Unknown => "unknown",
            },
            "retryable": refusal.retryable,
        }
    }))
}

/// A Discord nonce (at most 25 characters) for chunk `index` of request `id`.
#[must_use]
pub fn nonce(id: &str, index: usize) -> String {
    let compact: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(22)
        .collect();
    format!("{compact}{index:03}")
}

/// Handle one request. `transport` is called only after the request validates.
pub fn handle_request(
    input: &[u8],
    config: Result<&OutboundConfig, &str>,
    transport: &mut dyn FnMut() -> Result<Box<dyn DiscordRest>, String>,
) -> Vec<u8> {
    let input = trim_newline(input);
    let parsed: Option<Map<String, Value>> = if input.len() > MAX_REQUEST_BYTES {
        None
    } else {
        serde_json::from_slice(input).ok()
    };
    let Some(object) = parsed else {
        return respond_error(
            &Value::Null,
            &Value::Null,
            &Refusal::not_applied("request_invalid", "request is not one JSON object", false),
        );
    };
    let id = object.get("id").cloned().unwrap_or(Value::Null);
    let action = object.get("action").cloned().unwrap_or(Value::Null);
    let fail = |refusal: Refusal| respond_error(&id, &action, &refusal);
    if object.get("version") != Some(&json!(1)) || !id.is_string() {
        return fail(Refusal::not_applied(
            "request_invalid",
            "version must be 1 and id a string",
            false,
        ));
    }
    let config = match config {
        Ok(config) => config,
        Err(problem) => return fail(Refusal::not_applied("helper_misconfigured", problem, false)),
    };
    let request_id = id.as_str().unwrap_or_default();
    let field = |key: &str| object.get(key).and_then(Value::as_str);
    let channel = match field("channel_id").and_then(Snowflake::parse) {
        Some(channel) if config.channels.contains(&channel) => channel,
        Some(_) => {
            return fail(Refusal::not_applied(
                "channel_not_allowed",
                "channel is not in the helper's --channel list",
                false,
            ))
        }
        None => {
            return fail(Refusal::not_applied(
                "request_invalid",
                "channel_id must be a Discord channel id",
                false,
            ))
        }
    };
    match action.as_str() {
        Some("send") => {
            let reply_to = match object.get("thread_id") {
                None | Some(Value::Null) => None,
                Some(Value::String(thread)) => match ids::parse_thread_resource(thread) {
                    Some((thread_channel, root)) if thread_channel == channel => Some(root),
                    _ => {
                        return fail(Refusal::not_applied(
                            "request_invalid",
                            "thread_id is not <channel>/threads/<root> in this channel",
                            false,
                        ))
                    }
                },
                Some(_) => {
                    return fail(Refusal::not_applied(
                        "request_invalid",
                        "thread_id must be a string or null",
                        false,
                    ))
                }
            };
            let Some(text) = field("text").filter(|text| !text.trim().is_empty()) else {
                return fail(Refusal::not_applied(
                    "request_invalid",
                    "text must be a nonempty string",
                    false,
                ));
            };
            let prepared = match config.text_mode {
                TextMode::Mentions => markdown::neutralize_mentions(text),
                TextMode::Literal => markdown::escape_markdown(text),
                TextMode::Raw => text.to_owned(),
            };
            let Some(chunks) = markdown::chunk(&prepared, MAX_CONTENT_CHARS) else {
                return fail(Refusal::not_applied(
                    "too_long",
                    "a post with a fenced block exceeds 2000 characters and cannot be split",
                    false,
                ));
            };
            let mut rest = match transport() {
                Ok(rest) => rest,
                Err(problem) => {
                    return fail(Refusal::not_applied("helper_misconfigured", problem, true))
                }
            };
            let mut first = None;
            for (index, chunk) in chunks.iter().enumerate() {
                let reply = if index == 0 {
                    reply_to
                } else {
                    first.or(reply_to)
                };
                match rest.create_message(channel, chunk, reply, &nonce(request_id, index)) {
                    Ok(posted) => {
                        first.get_or_insert(posted);
                    }
                    Err(error) => {
                        let mut refusal = Refusal::from(error);
                        if first.is_some() {
                            // Part of the reply is already posted.
                            refusal.applied = Applied::Unknown;
                        }
                        return fail(refusal);
                    }
                }
            }
            let posted = first.expect("at least one chunk");
            line(&json!({
                "version": 1, "id": id, "action": action, "ok": true,
                "receipt": {"message_id": ids::message_resource(channel, posted)},
            }))
        }
        Some("ensure_reaction") => {
            let message = match field("message_id").and_then(ids::parse_message_resource) {
                Some((message_channel, message)) if message_channel == channel => message,
                _ => {
                    return fail(Refusal::not_applied(
                        "request_invalid",
                        "message_id is not <channel>/messages/<id> in this channel",
                        false,
                    ))
                }
            };
            let Some(emoji) = field("emoji").filter(|emoji| !emoji.is_empty() && emoji.len() <= 64)
            else {
                return fail(Refusal::not_applied(
                    "request_invalid",
                    "emoji must be a nonempty string",
                    false,
                ));
            };
            let mut rest = match transport() {
                Ok(rest) => rest,
                Err(problem) => {
                    return fail(Refusal::not_applied("helper_misconfigured", problem, true))
                }
            };
            match rest.add_reaction(channel, message, emoji) {
                Ok(()) => line(&json!({
                    "version": 1, "id": id, "action": action, "ok": true,
                    "receipt": {
                        "reaction_id": format!("{}/reactions/{emoji}", ids::message_resource(channel, message)),
                        "already_present": false,
                    },
                })),
                Err(error) => fail(Refusal::from(error)),
            }
        }
        _ => fail(Refusal::not_applied(
            "unsupported_action",
            "the Discord helper supports send and ensure_reaction",
            false,
        )),
    }
}

fn trim_newline(input: &[u8]) -> &[u8] {
    let mut end = input.len();
    while end > 0 && matches!(input[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &input[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_fits_discord_limit_and_differs_per_chunk() {
        let id = "0b8f1f3e-6c2a-4f55-9a0e-1234567890ab";
        assert_eq!(nonce(id, 0).len(), 25);
        assert_ne!(nonce(id, 0), nonce(id, 1));
        assert_eq!(nonce(id, 0), nonce(id, 0));
    }

    #[test]
    fn arguments_parse() {
        let config = OutboundConfig::parse(&[
            "--channel=1566504459499217097".to_owned(),
            "--token-file=/root/.claude/channels/discord/.env".to_owned(),
            "--text=literal".to_owned(),
        ])
        .expect("valid");
        assert_eq!(config.text_mode, TextMode::Literal);
        assert!(OutboundConfig::parse(&[]).is_err());
        assert!(
            OutboundConfig::parse(&["--channel=1".to_owned(), "--bogus=1".to_owned()]).is_err()
        );
    }
}
