//! The bridge's one-shot `outbound_command` helper for Discord.
//!
//! One request line in, one response line out, in the bridge's v1 outbound contract (the same
//! one the Slack helper speaks):
//!
//! ```json
//! {"version":1,"id":"<uuid>","action":"send","channel_id":"<chan>","thread_id":"<chan>/threads/<root>"|null,"text":"…"}
//! {"version":1,"id":"<uuid>","action":"ensure_reaction","channel_id":"<chan>","message_id":"<chan>/messages/<id>","emoji":"✅"}
//! {"version":1,"id":"<uuid>","action":"ensure_rejection","channel_id":"<chan>","message_id":"<chan>/messages/<id>","sender_id":"<user>","emoji":"❌"}
//! ```
//!
//! A `send` with a thread posts a Discord reply to the thread's root message (this is how relay
//! receipts land "in the post's thread"); with `thread_id: null` it posts at channel level.
//! Text over Discord's 2,000-character limit is split at line breaks. A fenced block crossing a
//! break is closed and reopened with its opener ([`markdown::chunk`]), except a relay block (a
//! line opening an `agent-teams-` fence): such a post is refused as `too_long` rather than
//! split, because its receiver needs the envelope whole. Configure the relay route's
//! `max_post_bytes` to about 1900 for Discord: neutralising mentions in the visible text adds a
//! few characters, and the limit applies after that.
//!
//! # ACK line (`--ack-host`)
//!
//! The bridge marks a request with its acknowledgement reaction only once it has durably
//! admitted it, through this helper's `ensure_reaction`. With `--ack-host=HOST` and
//! `--ack-bot-user-id=ID`, when that reaction is the acknowledgement (`--ack-emoji`, default the
//! bridge's built-in 👀) and succeeds, the helper also replies to the request with one line
//! naming the host and the agent: `ACK HOST/AGENT: received`. Discord attributes a reaction only
//! to the bot that added it, so this is what tells the owner which host took a post and for
//! whom. The agent comes from re-reading the post with the same addressing rule the plugin used
//! (`agent=NAME@HOST` names `NAME`; otherwise `--ack-bridge-agent`, default `bridge`). A reply
//! inside an existing reply chain names the host only, because the bridge may route it to an
//! agent bound to that thread. The line is best effort (a failure is logged and the reaction
//! response is unchanged) and its nonce is derived from the message. The helper reads the
//! request before reacting: when its own acknowledgement is already there (the bridge re-ran
//! the operation), it posts no second line, however long after, and reports
//! `already_present: true`. The read and the post share [`ACK_BUDGET`] (8 s) with per-call
//! timeouts cut to what is left and no rate-limit waits, so the line cannot push the helper past
//! the bridge's timeout.
//!
//! Every post sets `allowed_mentions.parse = []` and a nonce derived from the request id with
//! `enforce_nonce`, so a retried request does not post twice within Discord's nonce window.
//! Other actions (`update`, `delete`, `resolve_sender`, `list_members`) are refused as
//! `unsupported_action`, outcome `not_applied`.
//!
//! # Rate limits
//!
//! A call refused with HTTP 429 (or held by the transport because the route's bucket is empty)
//! is retried once after its `retry_after` when that is at most [`MAX_INLINE_WAIT`]; a longer
//! wait is returned as `rate_limited`, retryable, outcome `not_applied`, with the wait in the
//! detail. A 429 proves nothing was written, and a retried post keeps its nonce.
//!
//! # `ensure_rejection`
//!
//! The bridge asks for a rejection reaction on a post it refused (a sender who is not an owner,
//! or an `agent=` target this host refuses). The helper adds it only to a post whose author is
//! verifiably a person:
//!
//! 1. It binds the operation UUID, before any write, in `--ledger=DIR`'s
//!    `rejections/<UUID>.json`: channel, message, sender and the first emoji. The binding is
//!    created atomically (a hard link of a synced temporary file), so a concurrent run with the
//!    same UUID cannot bind different values. A UUID already bound to another channel, message
//!    or sender is refused as `operation_conflict`. A retry uses the bound emoji even after the
//!    configured one changes.
//! 2. It reads that exact message from Discord (`GET /channels/{c}/messages/{m}`) and requires
//!    the author id to equal `sender_id`, the author not marked `bot` or `system`, no
//!    `webhook_id`, and an ordinary message type. Otherwise, or when the message is gone, it
//!    answers `author_not_human`, nonretryable, `not_applied`, and records the refusal. The
//!    stream's own bot flag is never trusted for this.
//! 3. It adds the reaction (idempotent on Discord), records completion, and returns the
//!    standard reaction receipt. A completed UUID replays its receipt with
//!    `already_present: true` and contacts nothing.
//!
//! When an earlier attempt for the UUID may have written the reaction (its binding exists but
//! never completed), a later failure is reported with outcome `unknown`. Without `--ledger`
//! the action is refused as `helper_misconfigured` before Discord is contacted, so no
//! rejection reaction is ever added unguarded. Concurrent runs need no lock: the binding is
//! the only state they could disagree about, and adding the same reaction twice is a no-op.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::addressing::{AddressingPolicy, Post, Target, Verdict};
use crate::ids::{self, Snowflake};
use crate::markdown;
use crate::transport::{
    Applied, CurlTransport, DiscordMessage, DiscordRest, RestError, TokenSource, DEFAULT_API_BASE,
    MAX_CONTENT_CHARS,
};

/// Largest request accepted on standard input.
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
/// The longest rate-limit wait the helper sits out before retrying once.
pub const MAX_INLINE_WAIT: Duration = Duration::from_secs(5);
/// Schema of one rejection ledger record.
pub const REJECTION_SCHEMA: &str = "agentctl-discord-rejection/v1";

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
    /// Directory holding the rejection journal; required for `ensure_rejection`.
    pub ledger: Option<PathBuf>,
    /// The ACK line settings; `None` posts no ACK line.
    pub ack: Option<AckLine>,
}

/// Settings for the ACK line (see the module documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AckLine {
    /// This host's name.
    pub host: String,
    /// This host's bot user id.
    pub bot_user_id: Snowflake,
    /// This host's `@alias`es.
    pub aliases: BTreeSet<String>,
    /// The bridge's own target, as named in the line.
    pub bridge_agent: String,
    /// The bridge's acknowledgement emoji.
    pub emoji: String,
}

impl OutboundConfig {
    /// Parse `--channel=ID` (repeatable, required), `--token-file=PATH` [`--token-key=KEY`]
    /// or `--token-env=NAME`, `--curl=PATH`, `--api-base=URL`, `--timeout-seconds=N`,
    /// `--text=mentions|literal|raw`, `--ledger=DIR` (absolute; enables `ensure_rejection`),
    /// and for the ACK line `--ack-host=HOST` with `--ack-bot-user-id=ID`, `--ack-alias=A`
    /// (repeatable), `--ack-bridge-agent=NAME` and `--ack-emoji=E`.
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
        let mut ledger = None;
        let mut ack_host = None;
        let mut ack_bot = None;
        let mut ack_aliases = BTreeSet::new();
        let mut ack_bridge_agent = "bridge".to_owned();
        let mut ack_emoji = chat_subscription::reactions::DEFAULT_ACK_REACTION
            .emoji
            .to_owned();
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
                "--ledger" => {
                    let path = PathBuf::from(value);
                    if !path.is_absolute() {
                        return Err("--ledger must be an absolute directory".to_owned());
                    }
                    ledger = Some(path);
                }
                "--api-base" => value.clone_into(&mut api_base),
                "--ack-host" => ack_host = Some(label(value, "--ack-host")?),
                "--ack-bot-user-id" => {
                    ack_bot =
                        Some(Snowflake::parse(value).ok_or("--ack-bot-user-id must be a user id")?);
                }
                "--ack-alias" => {
                    ack_aliases.insert(label(value, "--ack-alias")?);
                }
                "--ack-bridge-agent" => ack_bridge_agent = label(value, "--ack-bridge-agent")?,
                "--ack-emoji" => {
                    if value.is_empty() || value.len() > 64 {
                        return Err("--ack-emoji must be one emoji".to_owned());
                    }
                    value.clone_into(&mut ack_emoji);
                }
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
        let ack = match (ack_host, ack_bot) {
            (Some(host), Some(bot_user_id)) => Some(AckLine {
                host,
                bot_user_id,
                aliases: ack_aliases,
                bridge_agent: ack_bridge_agent,
                emoji: ack_emoji,
            }),
            (None, None) => None,
            _ => return Err("--ack-host and --ack-bot-user-id go together".to_owned()),
        };
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
            ledger,
            ack,
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

fn label(value: &str, flag: &str) -> Result<String, String> {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "{flag} must be a name of letters, digits, '.', '_' or '-'"
        ))
    }
}

/// The ACK line for `message`, or `None` when the post is not an owner request addressed to
/// this host (the acknowledgement reaction went on something else).
#[must_use]
pub fn ack_text(ack: &AckLine, message: &DiscordMessage) -> Option<String> {
    let policy = AddressingPolicy {
        own_bot: ack.bot_user_id,
        host: ack.host.clone(),
        peer_bots: BTreeMap::new(),
        aliases: ack.aliases.clone(),
        local_agents: None,
    };
    let verdict = policy.classify(&Post {
        author: message.author_id,
        author_is_bot: !message.author_is_human(),
        content: &message.content,
    });
    let Verdict::Deliver(addressed) = verdict else {
        return None;
    };
    Some(match addressed.target {
        Target::Agent { name } => format!("ACK {}/{name}: received", ack.host),
        Target::Bridge if message.reply_to.is_some() => format!("ACK {}: received", ack.host),
        Target::Bridge => format!("ACK {}/{}: received", ack.host, ack.bridge_agent),
    })
}

/// The whole time the ACK line may add to an acknowledgement: its read of the request and its
/// post share it, and neither waits out a rate limit. With the reaction's own
/// `--timeout-seconds` (10 in the example) this stays well inside the bridge's 25 s helper
/// timeout.
pub const ACK_BUDGET: Duration = Duration::from_secs(8);

/// Seconds left before `deadline`, or `None` when under one second remains.
fn seconds_left(deadline: Instant) -> Option<u32> {
    let left = deadline.saturating_duration_since(Instant::now()).as_secs();
    u32::try_from(left).ok().filter(|left| *left >= 1)
}

/// Read the request before the acknowledgement reaction: whether this bot's reaction is already
/// there (a re-run of the same acknowledgement), and the ACK line to post after it, if any.
fn prepare_ack_line(
    rest: &mut dyn DiscordRest,
    ack: &AckLine,
    channel: Snowflake,
    message: Snowflake,
    deadline: Instant,
) -> (bool, Option<String>) {
    let Some(seconds) = seconds_left(deadline) else {
        return (false, None);
    };
    rest.set_timeout(seconds);
    match rest.get_message(channel, message) {
        Ok(found) => {
            let already = found.own_reactions.contains(&ack.emoji);
            // A re-run after the reaction landed posts no second line, even past Discord's
            // nonce window.
            (already, (!already).then(|| ack_text(ack, &found)).flatten())
        }
        Err(error) => {
            eprintln!("agentctl-discord-outbound: ACK line skipped, request unreadable: {error}");
            (false, None)
        }
    }
}

/// Post a prepared ACK line within what is left of its budget. Best effort: logged on failure.
fn post_ack_line(
    rest: &mut dyn DiscordRest,
    channel: Snowflake,
    message: Snowflake,
    text: &str,
    deadline: Instant,
) {
    let Some(seconds) = seconds_left(deadline) else {
        eprintln!("agentctl-discord-outbound: ACK line skipped, its time budget is spent");
        return;
    };
    rest.set_timeout(seconds);
    if let Err(error) = rest.create_message(channel, text, Some(message), &format!("ack{message}"))
    {
        eprintln!("agentctl-discord-outbound: ACK line not posted: {error}");
    }
}

/// Run `call`, and once more after a rate-limit wait of at most [`MAX_INLINE_WAIT`].
fn with_rate_retry<T>(mut call: impl FnMut() -> Result<T, RestError>) -> Result<T, RestError> {
    match call() {
        Err(error) if error.code == "rate_limited" => match error.retry_after {
            Some(wait) if wait <= MAX_INLINE_WAIT => {
                std::thread::sleep(wait + Duration::from_millis(50));
                call()
            }
            _ => Err(error),
        },
        other => other,
    }
}

/// One rejection ledger record.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RejectionRecord {
    channel: Snowflake,
    message: Snowflake,
    sender: Snowflake,
    emoji: String,
    /// `intent` (bound, a write may have happened), `refused` or `done`.
    state: String,
}

impl RejectionRecord {
    fn to_json(&self) -> Value {
        json!({
            "schema": REJECTION_SCHEMA,
            "channel": self.channel.to_string(),
            "message": self.message.to_string(),
            "sender": self.sender.to_string(),
            "emoji": self.emoji,
            "state": self.state,
        })
    }

    fn from_json(value: &Value) -> Option<Self> {
        if value.get("schema")? != REJECTION_SCHEMA {
            return None;
        }
        let snowflake = |key: &str| value.get(key)?.as_str().and_then(Snowflake::parse);
        let state = value.get("state")?.as_str()?;
        if !matches!(state, "intent" | "refused" | "done") {
            return None;
        }
        Some(Self {
            channel: snowflake("channel")?,
            message: snowflake("message")?,
            sender: snowflake("sender")?,
            emoji: value.get("emoji")?.as_str()?.to_owned(),
            state: state.to_owned(),
        })
    }
}

fn valid_operation_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn ledger_error(context: &str, error: &std::io::Error) -> Refusal {
    Refusal::not_applied("ledger_unavailable", format!("{context}: {error}"), true)
}

fn sync_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::File::open(directory)?.sync_all()
}

/// Write `record` to a synced temporary file in `directory` and return its path.
fn write_temporary(
    directory: &Path,
    id: &str,
    record: &RejectionRecord,
) -> std::io::Result<PathBuf> {
    let temporary = directory.join(format!(".{id}.{}.tmp", std::process::id()));
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(record.to_json().to_string().as_bytes())?;
    file.sync_all()?;
    Ok(temporary)
}

/// The rejection journal under one ledger directory.
struct RejectionJournal {
    directory: PathBuf,
}

impl RejectionJournal {
    fn open(ledger: &Path) -> Result<Self, Refusal> {
        let directory = ledger.join("rejections");
        std::fs::create_dir_all(&directory)
            .map_err(|error| ledger_error("creating the rejection journal", &error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| ledger_error("restricting the rejection journal", &error))?;
        }
        Ok(Self { directory })
    }

    fn path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }

    fn read(&self, id: &str) -> Result<Option<RejectionRecord>, Refusal> {
        match std::fs::read(self.path(id)) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .ok()
                .as_ref()
                .and_then(RejectionRecord::from_json)
                .map(Some)
                .ok_or_else(|| {
                    Refusal::not_applied(
                        "ledger_unavailable",
                        format!("rejection record {id} is unreadable"),
                        false,
                    )
                }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ledger_error("reading a rejection record", &error)),
        }
    }

    /// Bind `record` to `id` unless a binding exists; return the binding in force and whether
    /// it existed before this call.
    fn bind(&self, id: &str, record: &RejectionRecord) -> Result<(RejectionRecord, bool), Refusal> {
        if let Some(existing) = self.read(id)? {
            return Ok((existing, true));
        }
        let temporary = write_temporary(&self.directory, id, record)
            .map_err(|error| ledger_error("writing a rejection record", &error))?;
        let linked = std::fs::hard_link(&temporary, self.path(id));
        let _ = std::fs::remove_file(&temporary);
        match linked {
            Ok(()) => {
                sync_directory(&self.directory)
                    .map_err(|error| ledger_error("syncing the rejection journal", &error))?;
                Ok((record.clone(), false))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => self
                .read(id)?
                .map(|existing| (existing, true))
                .ok_or_else(|| ledger_error("reading a rejection record", &error)),
            Err(error) => Err(ledger_error("binding a rejection record", &error)),
        }
    }

    /// Replace `id`'s record with `record` in a new state.
    fn update(&self, id: &str, record: &RejectionRecord) -> Result<(), Refusal> {
        let temporary = write_temporary(&self.directory, id, record)
            .map_err(|error| ledger_error("writing a rejection record", &error))?;
        std::fs::rename(&temporary, self.path(id))
            .and_then(|()| sync_directory(&self.directory))
            .map_err(|error| ledger_error("updating a rejection record", &error))
    }
}

fn reaction_receipt(channel: Snowflake, message: Snowflake, emoji: &str, already: bool) -> Value {
    json!({
        "reaction_id": format!("{}/reactions/{emoji}", ids::message_resource(channel, message)),
        "already_present": already,
    })
}

/// The guarded rejection reaction (see the module documentation).
fn ensure_rejection(
    config: &OutboundConfig,
    id: &str,
    channel: Snowflake,
    message: Snowflake,
    sender: Snowflake,
    emoji: &str,
    transport: &mut dyn FnMut() -> Result<Box<dyn DiscordRest>, String>,
) -> Result<Value, Refusal> {
    let Some(ledger) = &config.ledger else {
        return Err(Refusal::not_applied(
            "helper_misconfigured",
            "ensure_rejection needs --ledger=DIR for its rejection journal",
            false,
        ));
    };
    if !valid_operation_id(id) {
        return Err(Refusal::not_applied(
            "request_invalid",
            "id must be 1-64 ASCII letters, digits or hyphens",
            false,
        ));
    }
    let journal = RejectionJournal::open(ledger)?;
    let (mut record, existed) = journal.bind(
        id,
        &RejectionRecord {
            channel,
            message,
            sender,
            emoji: emoji.to_owned(),
            state: "intent".to_owned(),
        },
    )?;
    if (record.channel, record.message, record.sender) != (channel, message, sender) {
        return Err(Refusal::not_applied(
            "operation_conflict",
            "this operation id is bound to another channel, message or sender",
            false,
        ));
    }
    match record.state.as_str() {
        "done" => return Ok(reaction_receipt(channel, message, &record.emoji, true)),
        "refused" => {
            return Err(Refusal::not_applied(
                "author_not_human",
                "the author was not verified as a person",
                false,
            ))
        }
        _ => {}
    }
    // An earlier attempt bound this operation and did not finish: it may have reacted.
    let uncertain = existed;
    let inherit = |mut refusal: Refusal| {
        if uncertain {
            refusal.applied = Applied::Unknown;
        }
        refusal
    };
    let mut rest = transport()
        .map_err(|problem| inherit(Refusal::not_applied("helper_misconfigured", problem, true)))?;
    let author = match with_rate_retry(|| rest.get_message(channel, message)) {
        Ok(found) => Some(found),
        Err(error) if error.code == "not_found" => None,
        Err(error) => return Err(inherit(Refusal::from(error))),
    };
    let human = author.is_some_and(|found| found.author_id == sender && found.author_is_human());
    if !human {
        if !uncertain {
            record.state = "refused".to_owned();
            journal.update(id, &record)?;
        }
        return Err(inherit(Refusal::not_applied(
            "author_not_human",
            "the message is gone, or its author is not this sender or is a bot, system user or webhook",
            false,
        )));
    }
    let bound_emoji = record.emoji.clone();
    with_rate_retry(|| rest.add_reaction(channel, message, &bound_emoji))
        .map_err(|error| inherit(Refusal::from(error)))?;
    record.state = "done".to_owned();
    journal.update(id, &record)?;
    Ok(reaction_receipt(channel, message, &bound_emoji, false))
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
                    "a relay post (an agent-teams- fence) exceeds 2000 characters and cannot be split",
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
                let chunk_nonce = nonce(request_id, index);
                match with_rate_retry(|| rest.create_message(channel, chunk, reply, &chunk_nonce)) {
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
            let ack_deadline = Instant::now() + ACK_BUDGET;
            let (already, ack_line) = match config.ack.as_ref().filter(|ack| ack.emoji == emoji) {
                Some(ack) => {
                    let prepared =
                        prepare_ack_line(rest.as_mut(), ack, channel, message, ack_deadline);
                    rest.set_timeout(config.timeout_seconds);
                    prepared
                }
                None => (false, None),
            };
            match with_rate_retry(|| rest.add_reaction(channel, message, emoji)) {
                Ok(()) => {
                    if let Some(text) = &ack_line {
                        post_ack_line(rest.as_mut(), channel, message, text, ack_deadline);
                    }
                    line(&json!({
                    "version": 1, "id": id, "action": action, "ok": true,
                    "receipt": reaction_receipt(channel, message, emoji, already),
                    }))
                }
                Err(error) => fail(Refusal::from(error)),
            }
        }
        Some("ensure_rejection") => {
            let expected = [
                "version",
                "id",
                "action",
                "channel_id",
                "message_id",
                "sender_id",
                "emoji",
            ];
            if object.len() != expected.len()
                || !expected.iter().all(|key| object.contains_key(*key))
            {
                return fail(Refusal::not_applied(
                    "request_invalid",
                    "a rejection request has exactly version, id, action, channel_id, message_id, sender_id and emoji",
                    false,
                ));
            }
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
            let Some(sender) = field("sender_id").and_then(Snowflake::parse) else {
                return fail(Refusal::not_applied(
                    "request_invalid",
                    "sender_id must be a Discord user id",
                    false,
                ));
            };
            let Some(emoji) = field("emoji").filter(|emoji| !emoji.is_empty() && emoji.len() <= 64)
            else {
                return fail(Refusal::not_applied(
                    "request_invalid",
                    "emoji must be a nonempty string",
                    false,
                ));
            };
            match ensure_rejection(
                config, request_id, channel, message, sender, emoji, transport,
            ) {
                Ok(receipt) => line(&json!({
                    "version": 1, "id": id, "action": action, "ok": true, "receipt": receipt,
                })),
                Err(refusal) => fail(refusal),
            }
        }
        _ => fail(Refusal::not_applied(
            "unsupported_action",
            "the Discord helper supports send, ensure_reaction and ensure_rejection",
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
    fn a_spent_ack_budget_makes_no_call() {
        use crate::transport::FakeRest;
        let ack = AckLine {
            host: "h".to_owned(),
            bot_user_id: Snowflake::from_u64(5),
            aliases: BTreeSet::new(),
            bridge_agent: "bridge".to_owned(),
            emoji: "👀".to_owned(),
        };
        let mut rest = FakeRest::default();
        let spent = Instant::now();
        let (channel, message) = (Snowflake::from_u64(1), Snowflake::from_u64(2));
        assert_eq!(
            prepare_ack_line(&mut rest, &ack, channel, message, spent),
            (false, None)
        );
        post_ack_line(&mut rest, channel, message, "ACK h/bridge: received", spent);
        assert_eq!(
            rest.calls, 0,
            "nothing is attempted once the budget is spent"
        );
        assert!(rest.posts.is_empty());
        // With budget left, the per-call timeout is cut to what remains.
        post_ack_line(
            &mut rest,
            channel,
            message,
            "ACK h/bridge: received",
            Instant::now() + Duration::from_millis(3_500),
        );
        assert_eq!(rest.timeouts, [3]);
        assert_eq!(rest.posts.len(), 1);
    }

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
