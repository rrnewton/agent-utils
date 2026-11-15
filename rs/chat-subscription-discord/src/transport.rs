//! Discord REST access.
//!
//! [`DiscordRest`] is the four calls the provider needs. [`CurlTransport`] implements them with
//! the system `curl`, passing the whole request configuration — URL, method, headers including
//! `Authorization: Bot <token>`, and the JSON body — as a curl config file on standard input
//! (`curl -K -`). The token therefore never appears in an argument list (visible to every
//! local user through `/proc`), and no error or log line in this crate ever includes it.
//! [`FakeRest`] is the deterministic in-memory transport the tests use.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use crate::ids::Snowflake;

/// Discord's default REST base.
pub const DEFAULT_API_BASE: &str = "https://discord.com/api/v10";
/// Discord's per-message content limit, in characters.
pub const MAX_CONTENT_CHARS: usize = 2000;
/// Discord's message `nonce` limit, in characters.
pub const MAX_NONCE_CHARS: usize = 25;

/// One Discord message, reduced to what the provider uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordMessage {
    /// Message id.
    pub id: Snowflake,
    /// Channel id.
    pub channel_id: Snowflake,
    /// Author user id.
    pub author_id: Snowflake,
    /// Whether the author is a bot account.
    pub author_is_bot: bool,
    /// Text content (empty when the bot lacks the message-content intent).
    pub content: String,
    /// ISO 8601 creation time as Discord reports it.
    pub timestamp: String,
    /// The message this one replies to, if any. Used for thread mapping only; a reply never
    /// addresses a post (see [`crate::addressing`]).
    pub reply_to: Option<Snowflake>,
}

/// Parse one Discord message object.
///
/// # Errors
///
/// Returns a description of the first missing or invalid field.
pub fn parse_message(value: &Value) -> Result<DiscordMessage, String> {
    let id = snowflake_field(value, "id")?;
    let channel_id = snowflake_field(value, "channel_id")?;
    let author = value.get("author").ok_or("message has no author")?;
    let author_id = snowflake_field(author, "id")?;
    let author_is_bot = author.get("bot").and_then(Value::as_bool).unwrap_or(false);
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .ok_or("message has no timestamp")?
        .to_owned();
    let reply_to = value
        .get("message_reference")
        .and_then(|reference| reference.get("message_id"))
        .and_then(Value::as_str)
        .and_then(Snowflake::parse);
    Ok(DiscordMessage {
        id,
        channel_id,
        author_id,
        author_is_bot,
        content,
        timestamp,
        reply_to,
    })
}

fn snowflake_field(value: &Value, field: &str) -> Result<Snowflake, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .and_then(Snowflake::parse)
        .ok_or_else(|| format!("message field {field} is not a snowflake"))
}

/// Whether a failed write may have taken effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Discord answered and the answer proves nothing was written.
    NotApplied,
    /// No definitive answer (timeout, transport failure, server error): it may have happened.
    Unknown,
}

/// A failed REST call. Never contains the token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestError {
    /// HTTP status, when one was received.
    pub status: Option<u16>,
    /// Lowercase machine code.
    pub code: String,
    /// Bounded detail.
    pub detail: String,
    /// Whether retrying may succeed.
    pub retryable: bool,
    /// Whether the call may have taken effect.
    pub applied: Applied,
}

impl RestError {
    fn new(code: &str, detail: impl Into<String>, retryable: bool, applied: Applied) -> Self {
        let mut detail = detail.into();
        if detail.len() > 400 {
            let mut end = 400;
            while !detail.is_char_boundary(end) {
                end -= 1;
            }
            detail.truncate(end);
        }
        Self {
            status: None,
            code: code.to_owned(),
            detail,
            retryable,
            applied,
        }
    }

    /// Classify an HTTP error status.
    #[must_use]
    pub fn from_status(status: u16, body: &str) -> Self {
        let (code, retryable, applied) = match status {
            429 => ("rate_limited", true, Applied::NotApplied),
            401 | 403 => ("forbidden", false, Applied::NotApplied),
            404 => ("not_found", false, Applied::NotApplied),
            400..=499 => ("request_refused", false, Applied::NotApplied),
            _ => ("server_error", true, Applied::Unknown),
        };
        let mut error = Self::new(code, format!("HTTP {status}: {body}"), retryable, applied);
        error.status = Some(status);
        error
    }
}

impl fmt::Display for RestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

/// The REST calls the provider makes.
pub trait DiscordRest: Send {
    /// Messages in `channel` strictly after `after` (or the newest ones when `None`), at most
    /// `limit` (1-100), in ascending id order.
    ///
    /// # Errors
    ///
    /// Any REST failure.
    fn list_messages(
        &mut self,
        channel: Snowflake,
        after: Option<Snowflake>,
        limit: u8,
    ) -> Result<Vec<DiscordMessage>, RestError>;

    /// Post `content` (at most [`MAX_CONTENT_CHARS`]) to `channel`, as a reply to `reply_to`
    /// when given. `nonce` makes Discord refuse a duplicate of a recent post with the same
    /// nonce (`enforce_nonce`), so a retried helper run does not post twice.
    ///
    /// # Errors
    ///
    /// Any REST failure.
    fn create_message(
        &mut self,
        channel: Snowflake,
        content: &str,
        reply_to: Option<Snowflake>,
        nonce: &str,
    ) -> Result<Snowflake, RestError>;

    /// Add this bot's `emoji` reaction (idempotent on Discord's side).
    ///
    /// # Errors
    ///
    /// Any REST failure.
    fn add_reaction(
        &mut self,
        channel: Snowflake,
        message: Snowflake,
        emoji: &str,
    ) -> Result<(), RestError>;
}

/// A bot token. Its `Debug` never shows the value.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// Wrap a token value.
    ///
    /// # Errors
    ///
    /// Refuses an empty token or one containing whitespace, quotes or backslashes (it would
    /// break the curl config line; real Discord tokens contain none).
    pub fn new(value: String) -> Result<Self, String> {
        if value.is_empty()
            || value
                .chars()
                .any(|c| c.is_whitespace() || c == '"' || c == '\\')
        {
            return Err("bot token is empty or contains invalid characters".to_owned());
        }
        Ok(Self(value))
    }

    /// Read `name` from the environment.
    ///
    /// # Errors
    ///
    /// The variable is unset or invalid. The message names the variable, never the value.
    pub fn from_env(name: &str) -> Result<Self, String> {
        let value = std::env::var(name).map_err(|_| format!("{name} is not set"))?;
        Self::new(value).map_err(|problem| format!("{name}: {problem}"))
    }

    /// Read `KEY=value` from a dotenv-style file (for example the Discord channel plugin's
    /// `.env`). Surrounding quotes are removed.
    ///
    /// # Errors
    ///
    /// The file is unreadable or has no valid `KEY=`. The message never includes the value.
    pub fn from_dotenv(path: &Path, key: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("reading {}: {error}", path.display()))?;
        for line in text.lines() {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            if let Some(value) = line
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
            {
                let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
                return Self::new(value.to_owned())
                    .map_err(|problem| format!("{key} in {}: {problem}", path.display()));
            }
        }
        Err(format!("{key} not found in {}", path.display()))
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(<redacted>)")
    }
}

/// Where the bot token comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenSource {
    /// An environment variable.
    Env(String),
    /// A `KEY=value` line in a dotenv file.
    DotEnv {
        /// File path.
        path: PathBuf,
        /// Variable name.
        key: String,
    },
}

impl TokenSource {
    /// Load the token.
    ///
    /// # Errors
    ///
    /// See [`Token::from_env`] and [`Token::from_dotenv`].
    pub fn load(&self) -> Result<Token, String> {
        match self {
            Self::Env(name) => Token::from_env(name),
            Self::DotEnv { path, key } => Token::from_dotenv(path, key),
        }
    }
}

/// The `curl`-backed transport.
#[derive(Debug)]
pub struct CurlTransport {
    curl: PathBuf,
    api_base: String,
    token: Token,
    timeout_seconds: u32,
}

impl CurlTransport {
    /// Build a transport. `curl` must be an absolute path.
    ///
    /// # Errors
    ///
    /// A relative `curl` path or a non-https API base.
    pub fn new(
        curl: PathBuf,
        api_base: String,
        token: Token,
        timeout_seconds: u32,
    ) -> Result<Self, String> {
        if !curl.is_absolute() {
            return Err("curl path must be absolute".to_owned());
        }
        if !api_base.starts_with("https://") {
            return Err("api base must be an https URL".to_owned());
        }
        Ok(Self {
            curl,
            api_base: api_base.trim_end_matches('/').to_owned(),
            token,
            timeout_seconds: timeout_seconds.clamp(1, 120),
        })
    }

    /// The curl config text for one request. Public for tests; it CONTAINS THE TOKEN and must
    /// never be logged.
    #[must_use]
    pub fn config_text(&self, method: &str, path: &str, body: Option<&Value>) -> String {
        let mut config = String::new();
        config.push_str(&format!(
            "url = \"{}\"\n",
            curl_quote(&format!("{}{path}", self.api_base))
        ));
        config.push_str(&format!("request = \"{method}\"\n"));
        config.push_str(&format!(
            "header = \"Authorization: Bot {}\"\n",
            self.token.0
        ));
        config.push_str("header = \"User-Agent: DiscordBot (agent-utils, 0.1)\"\n");
        if let Some(body) = body {
            config.push_str("header = \"Content-Type: application/json\"\n");
            config.push_str(&format!(
                "data-binary = \"{}\"\n",
                curl_quote(&body.to_string())
            ));
        } else if method == "PUT" {
            config.push_str("header = \"Content-Length: 0\"\n");
        }
        config
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, RestError> {
        let config = self.config_text(method, path, body);
        let mut child = Command::new(&self.curl)
            .args([
                "--silent",
                "--show-error",
                "--max-time",
                &self.timeout_seconds.to_string(),
                "--write-out",
                "\n%{http_code}",
                "--config",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                RestError::new(
                    "curl_unavailable",
                    error.to_string(),
                    true,
                    Applied::NotApplied,
                )
            })?;
        let written = child
            .stdin
            .take()
            .map(|mut stdin| stdin.write_all(config.as_bytes()));
        let output = child.wait_with_output().map_err(|error| {
            RestError::new("curl_failed", error.to_string(), true, Applied::Unknown)
        })?;
        if let Some(Err(error)) = written {
            return Err(RestError::new(
                "curl_failed",
                format!("writing the request: {error}"),
                true,
                Applied::NotApplied,
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let (body_text, status_text) = stdout.rsplit_once('\n').unwrap_or(("", &stdout));
        let status = status_text.trim().parse::<u16>().unwrap_or(0);
        if status == 0 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RestError::new(
                "transport_failed",
                format!("curl exit {:?}: {}", output.status.code(), stderr.trim()),
                true,
                Applied::Unknown,
            ));
        }
        if !(200..300).contains(&status) {
            return Err(RestError::from_status(status, body_text.trim()));
        }
        if body_text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(body_text).map_err(|error| {
            RestError::new(
                "invalid_response",
                format!("HTTP {status} body is not JSON: {error}"),
                false,
                Applied::Unknown,
            )
        })
    }
}

impl DiscordRest for CurlTransport {
    fn list_messages(
        &mut self,
        channel: Snowflake,
        after: Option<Snowflake>,
        limit: u8,
    ) -> Result<Vec<DiscordMessage>, RestError> {
        let limit = limit.clamp(1, 100);
        let path = match after {
            Some(after) => format!("/channels/{channel}/messages?after={after}&limit={limit}"),
            None => format!("/channels/{channel}/messages?limit={limit}"),
        };
        let value = self.call("GET", &path, None)?;
        let array = value.as_array().ok_or_else(|| {
            RestError::new(
                "invalid_response",
                "message list is not an array",
                false,
                Applied::NotApplied,
            )
        })?;
        let mut messages = Vec::with_capacity(array.len());
        for item in array {
            messages.push(parse_message(item).map_err(|problem| {
                RestError::new("invalid_response", problem, false, Applied::NotApplied)
            })?);
        }
        messages.sort_by_key(|message| message.id);
        Ok(messages)
    }

    fn create_message(
        &mut self,
        channel: Snowflake,
        content: &str,
        reply_to: Option<Snowflake>,
        nonce: &str,
    ) -> Result<Snowflake, RestError> {
        let body = create_message_body(content, reply_to, nonce);
        let value = self.call(
            "POST",
            &format!("/channels/{channel}/messages"),
            Some(&body),
        )?;
        value
            .get("id")
            .and_then(Value::as_str)
            .and_then(Snowflake::parse)
            .ok_or_else(|| {
                RestError::new(
                    "invalid_response",
                    "created message has no id",
                    false,
                    Applied::Unknown,
                )
            })
    }

    fn add_reaction(
        &mut self,
        channel: Snowflake,
        message: Snowflake,
        emoji: &str,
    ) -> Result<(), RestError> {
        let path = format!(
            "/channels/{channel}/messages/{message}/reactions/{}/@me",
            percent_encode(emoji)
        );
        self.call("PUT", &path, None).map(|_| ())
    }
}

/// The JSON body of a create-message call. Mentions in replies never ping anyone
/// (`allowed_mentions.parse = []`, `replied_user = false`): a bot reply that pinged the other
/// bot would read, to a careless filter, like addressing it.
#[must_use]
pub fn create_message_body(content: &str, reply_to: Option<Snowflake>, nonce: &str) -> Value {
    let mut body = json!({
        "content": content,
        "nonce": nonce.chars().take(MAX_NONCE_CHARS).collect::<String>(),
        "enforce_nonce": true,
        "allowed_mentions": {"parse": [], "replied_user": false},
    });
    if let Some(reply_to) = reply_to {
        body["message_reference"] =
            json!({"message_id": reply_to.to_string(), "fail_if_not_exists": false});
    }
    body
}

/// Escape for a double-quoted curl config value.
fn curl_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// One post recorded by [`FakeRest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakePost {
    /// Channel.
    pub channel: Snowflake,
    /// Content.
    pub content: String,
    /// Replied-to message.
    pub reply_to: Option<Snowflake>,
    /// Nonce.
    pub nonce: String,
}

/// Deterministic in-memory Discord for tests.
#[derive(Debug, Default)]
pub struct FakeRest {
    /// Messages per channel.
    pub channels: BTreeMap<Snowflake, Vec<DiscordMessage>>,
    /// Posts made through [`DiscordRest::create_message`].
    pub posts: Vec<FakePost>,
    /// Reactions added: (channel, message, emoji).
    pub reactions: Vec<(Snowflake, Snowflake, String)>,
    /// Number of list calls.
    pub list_calls: usize,
    /// Next id handed to a created message.
    pub next_id: u64,
    /// When set, every call fails with this error.
    pub fail_with: Option<RestError>,
}

impl FakeRest {
    /// Add a message to its channel.
    pub fn push(&mut self, message: DiscordMessage) {
        self.channels
            .entry(message.channel_id)
            .or_default()
            .push(message);
    }
}

impl DiscordRest for FakeRest {
    fn list_messages(
        &mut self,
        channel: Snowflake,
        after: Option<Snowflake>,
        limit: u8,
    ) -> Result<Vec<DiscordMessage>, RestError> {
        self.list_calls += 1;
        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }
        let mut messages: Vec<DiscordMessage> =
            self.channels.get(&channel).cloned().unwrap_or_default();
        messages.sort_by_key(|message| message.id);
        let limit = usize::from(limit.clamp(1, 100));
        Ok(match after {
            Some(after) => messages
                .into_iter()
                .filter(|message| message.id > after)
                .take(limit)
                .collect(),
            None => {
                let skip = messages.len().saturating_sub(limit);
                messages.into_iter().skip(skip).collect()
            }
        })
    }

    fn create_message(
        &mut self,
        channel: Snowflake,
        content: &str,
        reply_to: Option<Snowflake>,
        nonce: &str,
    ) -> Result<Snowflake, RestError> {
        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }
        self.next_id += 1;
        self.posts.push(FakePost {
            channel,
            content: content.to_owned(),
            reply_to,
            nonce: nonce.to_owned(),
        });
        Ok(Snowflake::from_u64(1_000_000 + self.next_id))
    }

    fn add_reaction(
        &mut self,
        channel: Snowflake,
        message: Snowflake,
        emoji: &str,
    ) -> Result<(), RestError> {
        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }
        self.reactions.push((channel, message, emoji.to_owned()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_rest_message_and_ignores_the_mentions_array() {
        let value = json!({
            "id": "1300000000000000002",
            "channel_id": "1566504459499217097",
            "author": {"id": "111222333444555666", "username": "owner"},
            "content": "thanks",
            "timestamp": "2026-11-15T16:45:00.123000+00:00",
            "mentions": [{"id": "1545796614826889226", "bot": true}],
            "message_reference": {"message_id": "1300000000000000001"}
        });
        let message = parse_message(&value).expect("parses");
        assert_eq!(message.id, Snowflake::from_u64(1_300_000_000_000_000_002));
        assert!(!message.author_is_bot);
        assert_eq!(
            message.reply_to,
            Some(Snowflake::from_u64(1_300_000_000_000_000_001))
        );
        assert_eq!(message.content, "thanks");
    }

    #[test]
    fn token_debug_is_redacted_and_config_quotes_body() {
        let token = Token::new("abc.DEF.ghi".to_owned()).expect("token");
        assert_eq!(format!("{token:?}"), "Token(<redacted>)");
        let transport = CurlTransport::new(
            PathBuf::from("/usr/bin/curl"),
            DEFAULT_API_BASE.to_owned(),
            token,
            10,
        )
        .expect("transport");
        assert!(!format!("{transport:?}").contains("abc.DEF.ghi"));
        let body = create_message_body("say \"hi\"\nnow", None, "n1");
        let config = transport.config_text("POST", "/channels/1/messages", Some(&body));
        assert!(config.contains("header = \"Authorization: Bot abc.DEF.ghi\"\n"));
        // The JSON body's quotes and the escaped newline survive curl's quoting.
        assert!(
            config.contains(r#"data-binary = "{\"allowed_mentions\""#),
            "{config}"
        );
        assert!(config.contains(r#"say \\\"hi\\\"\\nnow"#), "{config}");
        assert_eq!(config.lines().count(), 6);
    }

    #[test]
    fn tokens_refuse_config_breaking_characters() {
        for bad in ["", "a b", "a\"b", "a\\b", "a\nb"] {
            assert!(Token::new(bad.to_owned()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn dotenv_reads_the_named_key_without_echoing_it() {
        let dir = std::env::temp_dir().join(format!("discord-dotenv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join(".env");
        std::fs::write(&path, "OTHER=x\nDISCORD_BOT_TOKEN=\"tok.en\"\n").expect("write");
        let token = Token::from_dotenv(&path, "DISCORD_BOT_TOKEN").expect("token");
        assert_eq!(token.0, "tok.en");
        let missing = Token::from_dotenv(&path, "NOPE").expect_err("missing");
        assert!(!missing.contains("tok.en"));
        std::fs::remove_file(&path).expect("cleanup file");
        std::fs::remove_dir(&dir).expect("cleanup dir");
    }

    #[test]
    fn replies_never_ping() {
        let body = create_message_body(
            "x",
            Some(Snowflake::from_u64(5)),
            "0123456789012345678901234567",
        );
        assert_eq!(body["allowed_mentions"]["parse"], json!([]));
        assert_eq!(body["allowed_mentions"]["replied_user"], json!(false));
        assert_eq!(body["message_reference"]["message_id"], json!("5"));
        assert_eq!(body["nonce"].as_str().map(str::len), Some(MAX_NONCE_CHARS));
    }

    #[test]
    fn status_classification() {
        assert!(RestError::from_status(429, "").retryable);
        assert_eq!(RestError::from_status(403, "").applied, Applied::NotApplied);
        assert_eq!(RestError::from_status(502, "").applied, Applied::Unknown);
        assert_eq!(percent_encode("✅"), "%E2%9C%85");
    }
}
