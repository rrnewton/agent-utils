//! The bridge's one-shot `outbound_command` helper.
//!
//! Each run reads one request line and writes one response line, in the bridge's v1 outbound
//! contract:
//!
//! ```json
//! {"version":1,"id":"<uuid>","action":"send","channel_id":"C…","thread_id":"C…/threads/<ts>","text":"…"}
//! {"version":1,"id":"<uuid>","action":"ensure_reaction","channel_id":"C…","message_id":"C…/messages/<ts>","emoji":"🤖"}
//! ```
//!
//! A success names the posted message (`C…/messages/<ts>`) or reaction
//! (`C…/messages/<ts>/reactions/<name>`). A failure carries a lowercase code, a bounded detail,
//! whether the operation is proven `not_applied` or `unknown`, and whether retrying the same
//! UUID may succeed.
//!
//! # Bot identity only
//!
//! Every write acts as the configured Slack app (`--app-id=A…`). Without an app id the helper
//! refuses every request as not applied and contacts nothing.
//!
//! # Idempotency
//!
//! Slack has no idempotency key for posts, so the helper keeps a ledger directory with one small
//! file per operation UUID. Before a post it records an intent; after Slack returns the message
//! `ts` it records the result. A retry of a finished UUID returns the recorded message without
//! posting. A retry after an uncertain outcome first lists the target thread for a message from
//! this app, created after the intent, whose text matches; if one is found it is adopted. If none
//! is found (or the listing fails), the helper reports the outcome as still unknown until
//! `--uncertain-repost-after-seconds` (default 120) have passed since the intent, and then posts
//! again. A duplicate is therefore possible only when Slack accepted a post whose response was
//! lost and the listing still does not show it after that window.
//!
//! A body longer than `--max-chunk-bytes` (default 11,000; Slack accepts at most 12,000) is
//! posted as several consecutive messages in the same thread, split at line breaks where
//! possible. The receipt names the first message.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::cli::{CliOptions, CliTransport, TailOptions};
use crate::diag::Diagnostics;
use crate::ids::{self, AppId};
use crate::transport::{
    bounded, ListQuery, Outcome, PostMessage, SlackTransport, TextFormat, TransportError,
    WebApiTransport,
};

/// Largest request line accepted.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const LEDGER_VERSION: u64 = 1;
const MAX_LEDGER_BYTES: u64 = 64 * 1024;
const SLACK_MAX_BODY_BYTES: usize = 12_000;

/// Which transport the helper uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperTransport {
    /// An installed Slack client.
    Cli,
    /// The Slack Web API (not implemented; refuses).
    WebApi,
}

/// Validated helper arguments.
#[derive(Clone, Debug)]
pub struct OutboundConfig {
    /// Selected transport.
    pub transport: HelperTransport,
    /// Client path for the CLI transport.
    pub cli: Option<PathBuf>,
    /// Extra non-secret client environment.
    pub cli_environment: Vec<(String, String)>,
    /// The posting app; `None` refuses every request.
    pub app_id: Option<AppId>,
    /// Conversations the helper may write to.
    pub channels: Vec<String>,
    /// Operation ledger directory.
    pub ledger: Option<PathBuf>,
    /// Body format.
    pub format: TextFormat,
    /// Author IDs that identify this app's own posts during reconciliation.
    pub bot_user_ids: Vec<String>,
    /// How long an uncertain post stays unresolved before it is posted again.
    pub uncertain_repost_after: Duration,
    /// Deadline of one client invocation.
    pub timeout: Duration,
    /// Largest single message body.
    pub max_chunk_bytes: usize,
    /// Diagnostics file.
    pub diagnostics_log: Option<PathBuf>,
}

impl OutboundConfig {
    /// Parse `--name=value` helper arguments.
    ///
    /// # Errors
    ///
    /// Returns a description of the first invalid or unknown argument. A missing app id is not
    /// an argument error: it is reported on each request as `app_identity_required`.
    pub fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut config = Self {
            transport: HelperTransport::Cli,
            cli: None,
            cli_environment: Vec::new(),
            app_id: None,
            channels: Vec::new(),
            ledger: None,
            format: TextFormat::Markdown,
            bot_user_ids: Vec::new(),
            uncertain_repost_after: Duration::from_secs(120),
            timeout: Duration::from_millis(25_000),
            max_chunk_bytes: 11_000,
            diagnostics_log: None,
        };
        for argument in arguments {
            let (name, value) = argument
                .split_once('=')
                .ok_or_else(|| format!("argument {argument:?} is not --name=value"))?;
            match name {
                "--transport" => {
                    config.transport = match value {
                        "cli" => HelperTransport::Cli,
                        "web-api" => HelperTransport::WebApi,
                        _ => return Err("--transport must be cli or web-api".to_owned()),
                    };
                }
                "--cli" => {
                    let path = PathBuf::from(value);
                    if !path.is_absolute() {
                        return Err("--cli must be an absolute path".to_owned());
                    }
                    config.cli = Some(path);
                }
                "--cli-env" => {
                    let (key, item) = value
                        .split_once('=')
                        .ok_or_else(|| "--cli-env takes NAME=VALUE".to_owned())?;
                    if key.is_empty()
                        || !key
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    {
                        return Err("--cli-env name must match [A-Za-z0-9_]+".to_owned());
                    }
                    config
                        .cli_environment
                        .push((key.to_owned(), item.to_owned()));
                }
                "--app-id" => config.app_id = Some(AppId::new(value)?),
                "--channel" => {
                    if !ids::valid_channel_id(value) {
                        return Err(format!(
                            "--channel {value:?} is not a Slack conversation ID"
                        ));
                    }
                    config.channels.push(value.to_owned());
                }
                "--ledger" => {
                    let path = PathBuf::from(value);
                    if !path.is_absolute() {
                        return Err("--ledger must be an absolute path".to_owned());
                    }
                    config.ledger = Some(path);
                }
                "--format" => {
                    config.format = match value {
                        "markdown" => TextFormat::Markdown,
                        "text" => TextFormat::Text,
                        _ => return Err("--format must be markdown or text".to_owned()),
                    };
                }
                "--bot-user-id" => {
                    if !ids::valid_author_id(value) {
                        return Err("--bot-user-id must be a Slack U…, W… or B… id".to_owned());
                    }
                    config.bot_user_ids.push(value.to_owned());
                }
                "--uncertain-repost-after-seconds" => {
                    let seconds: u64 = value
                        .parse()
                        .ok()
                        .filter(|seconds| (30..=86_400).contains(seconds))
                        .ok_or_else(|| {
                            "--uncertain-repost-after-seconds must be 30 to 86400".to_owned()
                        })?;
                    config.uncertain_repost_after = Duration::from_secs(seconds);
                }
                "--timeout-ms" => {
                    let millis: u64 = value
                        .parse()
                        .ok()
                        .filter(|millis| (1_000..=29_000).contains(millis))
                        .ok_or_else(|| "--timeout-ms must be 1000 to 29000".to_owned())?;
                    config.timeout = Duration::from_millis(millis);
                }
                "--max-chunk-bytes" => {
                    config.max_chunk_bytes = value
                        .parse()
                        .ok()
                        .filter(|bytes| (1_000..=SLACK_MAX_BODY_BYTES).contains(bytes))
                        .ok_or_else(|| "--max-chunk-bytes must be 1000 to 12000".to_owned())?;
                }
                "--diagnostics-log" => config.diagnostics_log = Some(PathBuf::from(value)),
                _ => return Err(format!("unknown argument {name}")),
            }
        }
        if config.channels.is_empty() {
            return Err("at least one --channel is required".to_owned());
        }
        if config.ledger.is_none() {
            return Err("--ledger is required so uncertain posts can be reconciled".to_owned());
        }
        if config.transport == HelperTransport::Cli && config.cli.is_none() {
            return Err("--cli is required for the cli transport".to_owned());
        }
        Ok(config)
    }

    /// Build the selected transport.
    ///
    /// # Errors
    ///
    /// Returns a description when the CLI transport cannot be configured.
    pub fn transport(&self) -> Result<Box<dyn SlackTransport>, String> {
        match self.transport {
            HelperTransport::WebApi => Ok(Box::new(WebApiTransport)),
            HelperTransport::Cli => CliTransport::new(CliOptions {
                program: self.cli.clone().ok_or("--cli is required")?,
                environment: self.cli_environment.clone(),
                timeout: self.timeout,
                tail: TailOptions::default(),
            })
            .map(|transport| Box::new(transport) as Box<dyn SlackTransport>),
        }
    }
}

/// A failure response before or instead of contacting Slack.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Refusal {
    code: String,
    detail: String,
    outcome: Outcome,
    retryable: bool,
}

impl Refusal {
    fn not_applied(code: &str, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            detail: bounded(&detail.into()),
            outcome: Outcome::NotApplied,
            retryable,
        }
    }

    fn unknown(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            detail: bounded(&detail.into()),
            outcome: Outcome::Unknown,
            retryable: true,
        }
    }
}

impl From<TransportError> for Refusal {
    fn from(error: TransportError) -> Self {
        Self {
            code: error.code,
            detail: error.detail,
            outcome: error.outcome,
            retryable: error.retryable,
        }
    }
}

fn failure_line(id: Value, action: Value, refusal: &Refusal) -> Vec<u8> {
    let outcome = match refusal.outcome {
        Outcome::NotApplied => "not_applied",
        Outcome::Unknown => "unknown",
    };
    line(&json!({
        "version": 1,
        "id": id,
        "action": action,
        "ok": false,
        "error": {
            "code": crate::transport::slug(&refusal.code),
            "detail": bounded(&refusal.detail),
            "outcome": outcome,
            "retryable": refusal.retryable,
        }
    }))
}

fn line(value: &Value) -> Vec<u8> {
    let mut encoded = serde_json::to_vec(value).unwrap_or_default();
    encoded.push(b'\n');
    encoded
}

/// A lowercase RFC 4122 version-4 UUID, as the bridge issues.
#[must_use]
pub fn valid_operation_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            14 => byte == b'4',
            19 => matches!(byte, b'8' | b'9' | b'a' | b'b'),
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        })
}

enum Request {
    Send {
        channel_id: String,
        thread_ts: Option<String>,
        text: String,
    },
    Reaction {
        channel_id: String,
        message_ts: String,
        emoji: String,
    },
}

fn exact_keys(object: &Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
}

fn parse_request(object: &Map<String, Value>, action: &str) -> Result<Request, Refusal> {
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
    let invalid = |detail: &str| Refusal::not_applied("request_invalid", detail, false);
    match action {
        "send" => {
            if !exact_keys(
                object,
                &["version", "id", "action", "channel_id", "thread_id", "text"],
            ) {
                return Err(invalid("a send request has exactly version, id, action, channel_id, thread_id and text"));
            }
            let channel_id =
                text("channel_id").ok_or_else(|| invalid("channel_id must be a string"))?;
            let thread_ts = match object.get("thread_id") {
                Some(Value::Null) => None,
                Some(Value::String(thread)) => {
                    let (channel, ts) = ids::parse_resource(thread, "threads")
                        .ok_or_else(|| invalid("thread_id is not <channel>/threads/<ts>"))?;
                    if channel != channel_id {
                        return Err(invalid("thread_id belongs to another channel"));
                    }
                    Some(ts.to_owned())
                }
                _ => return Err(invalid("thread_id must be a string or null")),
            };
            let body = text("text").ok_or_else(|| invalid("text must be a string"))?;
            if body.trim().is_empty() {
                return Err(invalid("text is empty"));
            }
            Ok(Request::Send {
                channel_id,
                thread_ts,
                text: body,
            })
        }
        "ensure_reaction" => {
            if !exact_keys(
                object,
                &[
                    "version",
                    "id",
                    "action",
                    "channel_id",
                    "message_id",
                    "emoji",
                ],
            ) {
                return Err(invalid("a reaction request has exactly version, id, action, channel_id, message_id and emoji"));
            }
            let channel_id =
                text("channel_id").ok_or_else(|| invalid("channel_id must be a string"))?;
            let message =
                text("message_id").ok_or_else(|| invalid("message_id must be a string"))?;
            let (channel, ts) = ids::parse_resource(&message, "messages")
                .ok_or_else(|| invalid("message_id is not <channel>/messages/<ts>"))?;
            if channel != channel_id {
                return Err(invalid("message_id belongs to another channel"));
            }
            Ok(Request::Reaction {
                channel_id,
                message_ts: ts.to_owned(),
                emoji: text("emoji").ok_or_else(|| invalid("emoji must be a string"))?,
            })
        }
        _ => Err(invalid("action must be send or ensure_reaction")),
    }
}

/// Handle one request line and return exactly one response line.
///
/// `config` is the parsed helper configuration or the argument error. `transport` is built only
/// for a request that passed every local check, so a refused request never starts the client.
pub fn handle_request(
    input: &[u8],
    config: Result<&OutboundConfig, &str>,
    transport: &mut dyn FnMut() -> Result<Box<dyn SlackTransport>, String>,
    now_millis: &dyn Fn() -> u64,
) -> Vec<u8> {
    let parsed = (input.len() <= MAX_REQUEST_BYTES)
        .then(|| chat_subscription_plugin::decode_strict_json::<Value>(trim_newline(input)).ok())
        .flatten();
    let Some(Value::Object(object)) = parsed else {
        return failure_line(
            Value::Null,
            Value::Null,
            &Refusal::not_applied(
                "request_invalid",
                "the request is not one strict JSON object line",
                false,
            ),
        );
    };
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_operation_uuid(id));
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .filter(|action| matches!(*action, "send" | "ensure_reaction"));
    let (Some(id), Some(action)) = (id, action) else {
        return failure_line(
            Value::Null,
            Value::Null,
            &Refusal::not_applied(
                "request_invalid",
                "the request lacks a valid UUID id or a known action",
                false,
            ),
        );
    };
    let respond_error = |refusal: &Refusal| failure_line(json!(id), json!(action), refusal);
    if object.get("version").and_then(Value::as_u64) != Some(1) {
        return respond_error(&Refusal::not_applied(
            "request_invalid",
            "only version 1 is supported",
            false,
        ));
    }
    let config = match config {
        Ok(config) => config,
        Err(problem) => {
            return respond_error(&Refusal::not_applied(
                "helper_misconfigured",
                format!("the outbound helper arguments are invalid: {problem}"),
                true,
            ));
        }
    };
    let diagnostics = Diagnostics::new(config.diagnostics_log.clone(), "slack-outbound");
    let Some(app_id) = config.app_id.as_ref() else {
        diagnostics.log(&format!("refused {action} {id}: no --app-id configured"));
        return respond_error(&Refusal::not_applied(
            "app_identity_required",
            "no Slack app id is configured; the helper never posts as a user",
            true,
        ));
    };
    let request = match parse_request(&object, action) {
        Ok(request) => request,
        Err(refusal) => return respond_error(&refusal),
    };
    let channel = match &request {
        Request::Send { channel_id, .. } | Request::Reaction { channel_id, .. } => {
            channel_id.clone()
        }
    };
    if !config.channels.contains(&channel) {
        return respond_error(&Refusal::not_applied(
            "channel_not_allowed",
            format!("channel {channel} is not in this helper's --channel list"),
            false,
        ));
    }
    let mut transport = match transport() {
        Ok(transport) => transport,
        Err(problem) => {
            return respond_error(&Refusal::not_applied("helper_misconfigured", problem, true));
        }
    };
    let result = match request {
        Request::Send {
            channel_id,
            thread_ts,
            text,
        } => send(
            config,
            app_id,
            transport.as_mut(),
            id,
            &channel_id,
            thread_ts.as_deref(),
            &text,
            now_millis,
            &diagnostics,
        )
        .map(|message_id| json!({"message_id": message_id})),
        Request::Reaction {
            channel_id,
            message_ts,
            emoji,
        } => match ids::reaction_name(&emoji) {
            None => Err(Refusal::not_applied(
                "reaction_unsupported",
                "the configured reaction has no Slack shortcode mapping; configure a shortcode",
                false,
            )),
            Some(name) => transport
                .add_reaction(app_id, &channel_id, &message_ts, &name)
                .map(|already_present| {
                    diagnostics.log(&format!("reaction {id}: {name} already_present={already_present}"));
                    json!({
                        "reaction_id": format!("{}/reactions/{name}", ids::message_resource(&channel_id, &message_ts)),
                        "already_present": already_present,
                    })
                })
                .map_err(|error| {
                    diagnostics.log(&format!("reaction {id} failed: {error}"));
                    Refusal::from(error)
                }),
        },
    };
    match result {
        Ok(receipt) => line(&json!({
            "version": 1,
            "id": id,
            "action": action,
            "ok": true,
            "receipt": receipt,
        })),
        Err(refusal) => respond_error(&refusal),
    }
}

fn trim_newline(input: &[u8]) -> &[u8] {
    let mut end = input.len();
    while end > 0 && matches!(input[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &input[..end]
}

/// Split a body into chunks of at most `limit` bytes, preferring line breaks.
#[must_use]
pub fn split_body(text: &str, limit: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while rest.len() > limit {
        let mut boundary = limit;
        while !rest.is_char_boundary(boundary) {
            boundary -= 1;
        }
        let cut = rest[..boundary]
            .rfind('\n')
            .filter(|index| *index >= limit / 2)
            .map_or(boundary, |index| index + 1);
        chunks.push(rest[..cut].to_owned());
        rest = &rest[cut..];
    }
    if !rest.trim().is_empty() || chunks.is_empty() {
        chunks.push(rest.to_owned());
    }
    chunks
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LedgerChunk {
    ts: Option<String>,
    intent_millis: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LedgerRecord {
    channel_id: String,
    thread_ts: Option<String>,
    text_bytes: usize,
    chunks: Vec<LedgerChunk>,
}

impl LedgerRecord {
    fn to_json(&self) -> Value {
        json!({
            "version": LEDGER_VERSION,
            "channel_id": self.channel_id,
            "thread_ts": self.thread_ts,
            "text_bytes": self.text_bytes,
            "chunks": self.chunks.iter().map(|chunk| json!({
                "ts": chunk.ts,
                "intent_millis": chunk.intent_millis,
            })).collect::<Vec<_>>(),
        })
    }

    fn from_json(value: &Value) -> Option<Self> {
        if value.get("version")?.as_u64()? != LEDGER_VERSION {
            return None;
        }
        let chunks = value
            .get("chunks")?
            .as_array()?
            .iter()
            .map(|chunk| {
                Some(LedgerChunk {
                    ts: match chunk.get("ts")? {
                        Value::Null => None,
                        Value::String(ts) if ids::valid_ts(ts) => Some(ts.clone()),
                        _ => return None,
                    },
                    intent_millis: match chunk.get("intent_millis")? {
                        Value::Null => None,
                        other => Some(other.as_u64()?),
                    },
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            channel_id: value.get("channel_id")?.as_str()?.to_owned(),
            thread_ts: match value.get("thread_ts")? {
                Value::Null => None,
                other => Some(other.as_str()?.to_owned()),
            },
            text_bytes: usize::try_from(value.get("text_bytes")?.as_u64()?).ok()?,
            chunks,
        })
    }
}

fn ledger_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.json"))
}

fn load_ledger(directory: &Path, id: &str) -> Result<Option<LedgerRecord>, Refusal> {
    let path = ledger_path(directory, id);
    match fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(Refusal::not_applied(
                "ledger_unreadable",
                error.to_string(),
                true,
            ))
        }
        Ok(metadata) if metadata.len() > MAX_LEDGER_BYTES => {
            return Err(Refusal::unknown(
                "ledger_unreadable",
                "the operation ledger record is oversized",
            ));
        }
        Ok(_) => {}
    }
    let bytes = fs::read(&path)
        .map_err(|error| Refusal::unknown("ledger_unreadable", error.to_string()))?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .as_ref()
        .and_then(LedgerRecord::from_json)
        .map(Some)
        .ok_or_else(|| {
            Refusal::unknown(
                "ledger_unreadable",
                "the operation ledger record is malformed",
            )
        })
}

fn save_ledger(directory: &Path, id: &str, record: &LedgerRecord) -> Result<(), Refusal> {
    let fail =
        |error: std::io::Error| Refusal::not_applied("ledger_unwritable", error.to_string(), true);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(fail)?;
    let path = ledger_path(directory, id);
    let temporary = directory.join(format!(".{id}.json.{}", std::process::id()));
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(fail)?;
        file.write_all(&serde_json::to_vec(&record.to_json()).unwrap_or_default())
            .and_then(|()| file.sync_all())
            .map_err(fail)?;
    }
    fs::rename(&temporary, &path).map_err(fail)
}

/// Letters and digits of the first `limit` characters' worth of visible text, lowercase, with
/// Markdown link targets removed, so a Markdown body can be compared with its plain rendering.
fn fingerprint(text: &str, limit: usize) -> String {
    let mut visible = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        visible.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        rest = rest.find(')').map_or("", |end| &rest[end + 1..]);
    }
    visible.push_str(rest);
    visible
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .take(limit)
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn send(
    config: &OutboundConfig,
    app_id: &AppId,
    transport: &mut dyn SlackTransport,
    id: &str,
    channel_id: &str,
    thread_ts: Option<&str>,
    text: &str,
    now_millis: &dyn Fn() -> u64,
    diagnostics: &Diagnostics,
) -> Result<String, Refusal> {
    let directory = config.ledger.as_deref().ok_or_else(|| {
        Refusal::not_applied("helper_misconfigured", "--ledger is required", true)
    })?;
    let chunks = split_body(text, config.max_chunk_bytes);
    let mut record = match load_ledger(directory, id)? {
        Some(record) => {
            if record.channel_id != channel_id
                || record.thread_ts.as_deref() != thread_ts
                || record.text_bytes != text.len()
                || record.chunks.len() != chunks.len()
            {
                return Err(Refusal::not_applied(
                    "operation_conflict",
                    "this operation id was already used for a different message",
                    false,
                ));
            }
            record
        }
        None => LedgerRecord {
            channel_id: channel_id.to_owned(),
            thread_ts: thread_ts.map(str::to_owned),
            text_bytes: text.len(),
            chunks: vec![
                LedgerChunk {
                    ts: None,
                    intent_millis: None,
                };
                chunks.len()
            ],
        },
    };
    for (index, chunk) in chunks.iter().enumerate() {
        if record.chunks[index].ts.is_some() {
            continue;
        }
        if let Some(intent) = record.chunks[index].intent_millis {
            let oldest = ids::ts_from_millis(intent.saturating_sub(2_000));
            // A listing failure (for example an app without history scope) is treated like an
            // empty listing: the uncertainty window below still bounds how long the reply waits.
            let listed = match transport.list_messages(&ListQuery {
                app_id,
                channel_id,
                thread_ts,
                oldest: &oldest,
            }) {
                Ok(listed) => listed,
                Err(error) => {
                    diagnostics.log(&format!(
                        "send {id} chunk {index}: reconciliation listing failed: {error}"
                    ));
                    Vec::new()
                }
            };
            let wanted = fingerprint(chunk, 48);
            let taken: Vec<&str> = record
                .chunks
                .iter()
                .filter_map(|chunk| chunk.ts.as_deref())
                .collect();
            let adopted = listed.into_iter().find(|message| {
                (message.is_author_bot
                    || message
                        .author_user_id
                        .as_ref()
                        .is_some_and(|author| config.bot_user_ids.contains(author)))
                    && !taken.contains(&message.ts.as_str())
                    && fingerprint(&message.content, 48) == wanted
            });
            if let Some(message) = adopted {
                diagnostics.log(&format!(
                    "send {id} chunk {index}: adopted earlier post {}",
                    message.ts
                ));
                record.chunks[index].ts = Some(message.ts);
                record.chunks[index].intent_millis = None;
                save_ledger(directory, id, &record)?;
                continue;
            }
            let age = now_millis().saturating_sub(intent);
            if u128::from(age) < config.uncertain_repost_after.as_millis() {
                return Err(Refusal::unknown(
                    "outcome_unresolved",
                    format!(
                        "an earlier attempt's outcome is unknown and no matching post is listed yet ({} s old)",
                        age / 1_000
                    ),
                ));
            }
            diagnostics.log(&format!(
                "send {id} chunk {index}: no earlier post found after {} s; posting again",
                age / 1_000
            ));
        }
        record.chunks[index].intent_millis = Some(now_millis());
        save_ledger(directory, id, &record)?;
        let posted = transport.post_message(&PostMessage {
            app_id,
            channel_id,
            thread_ts,
            text: chunk,
            format: config.format,
        });
        match posted {
            Ok(ts) if ids::valid_ts(&ts) => {
                diagnostics.log(&format!(
                    "send {id} chunk {index}: posted {channel_id} {ts}"
                ));
                record.chunks[index].ts = Some(ts);
                record.chunks[index].intent_millis = None;
                save_ledger(directory, id, &record)?;
            }
            Ok(_) => {
                return Err(Refusal::unknown(
                    "receipt_invalid",
                    "the transport returned an invalid message ts",
                ));
            }
            Err(error) => {
                diagnostics.log(&format!("send {id} chunk {index} failed: {error}"));
                if error.outcome == Outcome::NotApplied {
                    record.chunks[index].intent_millis = None;
                    save_ledger(directory, id, &record)?;
                }
                return Err(Refusal::from(error));
            }
        }
    }
    let first = record.chunks[0]
        .ts
        .as_deref()
        .ok_or_else(|| Refusal::unknown("receipt_invalid", "no message ts was recorded"))?;
    Ok(ids::message_resource(channel_id, first))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_long_bodies_at_line_breaks() {
        let body = format!("{}\n{}", "a".repeat(1_500), "b".repeat(1_500));
        let chunks = split_body(&body, 2_000);
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 2_000));
        assert_eq!(chunks.concat(), body);
        assert_eq!(split_body("short", 2_000), vec!["short".to_owned()]);
    }

    #[test]
    fn fingerprints_ignore_markdown() {
        assert_eq!(
            fingerprint("**[label]** see [docs](https://example.com/x) now", 48),
            fingerprint("[label] see docs now", 48)
        );
    }

    #[test]
    fn parses_arguments() {
        let arguments = [
            "--cli=/usr/bin/slack",
            "--app-id=A0BRVKV248L",
            "--channel=C0C91RW5FAR",
            "--ledger=/var/lib/x",
        ]
        .map(str::to_owned);
        let config = OutboundConfig::parse(&arguments).expect("valid");
        assert_eq!(
            config.app_id.as_ref().map(AppId::as_str),
            Some("A0BRVKV248L")
        );
        assert!(OutboundConfig::parse(&["--as-user=1".to_owned()]).is_err());
        assert!(OutboundConfig::parse(&arguments[..3]).is_err());
    }
}
