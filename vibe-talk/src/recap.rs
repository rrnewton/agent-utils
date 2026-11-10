//! `vibe-talk recap`: a window of chat history labelled with what the owner has read.
//! `#717 vt-unread-recap`.
//!
//! The owner marks messages Done as reading progress (the per-message dismissal behind
//! `POST /api/v1/channels/{id}/dismiss` and `/restore`). This module is the client half of the
//! recap: it talks to a running server's `GET /api/v1/channels/{id}/recap` with the READ token
//! and prints the window oldest first, grouped by thread, every message tagged DONE or UNREAD.
//! The wire types and the grouping live here too, so the server
//! ([`crate::ops::recap`]) and this client cannot drift apart.
//!
//! Read state semantics, stated once: DONE is per message, never per thread. A thread root and
//! each of its replies carry independent marks under the parent channel's id. The per-channel
//! read mark (`ReadMark.last_read`) is secondary — shown in the header, never used to tag a
//! message.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::model::{ChannelInfo, Message, MessageId};

/// Help for `vibe-talk recap --help`.
pub const USAGE: &str = "\
vibe-talk recap — print a window of chat history labelled DONE / UNREAD

USAGE:
    vibe-talk recap [--config PATH] [--config-overlay PATH]...
                    [--hours N | --since <permalink|message id|time>]
                    [--channel <id|label|alias>] [--max-chars N] [--json]
                    [--server URL]

Prints chat history from a running vibe-talk server, oldest first, grouped by thread, each
message tagged DONE (the owner has marked it read) or UNREAD. Times are shown in ET
(America/New_York). The READ token is read from the server's own configuration (the file, then
VIBE_TALK_READ_TOKEN, which wins) and is never printed. The server is reached on the
configuration's [server] bind address; 0.0.0.0 and [::] are reached on loopback.

OPTIONS:
    --config PATH          the server's configuration file, found exactly as the server finds it
                           (default: $VIBE_TALK_CONFIG, else ./vibe-talk.toml; with neither, the
                           configuration comes from the environment alone)
    --config-overlay PATH  a TOML file laid over the configuration, as for the server. Repeatable;
                           VIBE_TALK_CONFIG_OVERLAY=PATH[:PATH...] is applied after the flags
    --hours N              window of the last N hours, 1 to 168 (default: 24 when neither
                           --hours nor --since is given)
    --since WHEN           window from a permalink, a message id, or a time: an RFC 3339
                           instant, or YYYY-MM-DD or \"YYYY-MM-DD HH:MM\" in ET. A permalink
                           also selects its channel unless --channel is given. The window
                           starts at that message (or instant), inclusive
    --channel WHICH        only this channel: its id, label, or alias. Default: every channel
                           the read token sees, in the server's list order
    --max-chars N          markdown budget in characters (default: 24000; 0 means no limit).
                           Over budget, DONE messages shrink to one line, then to their header
                           line; UNREAD text is never cut
    --json                 print the same data as structured JSON instead of markdown
    --server URL           reach the server at this http(s) address instead of the
                           configuration's bind address (for a test server)
    --help                 print this message and exit

EXAMPLES:
    vibe-talk recap
    vibe-talk recap --hours 12 --channel \"lead team\"
    vibe-talk recap --since 2026-10-30 --json
    vibe-talk recap --since https://discord.com/channels/123/456/789
";

/// The footer handed to whatever agent reads the markdown.
pub const FOOTER: &str = "\
---
For the reading agent: do not repeat DONE content unless its status changed later in the \
window; count superseded transient statuses only in their final state. Prefer linking \
permalinks over quoting text the owner has already read.";

/// How many replies of a pre-window thread are shown as CONTEXT.
pub const CONTEXT_REPLIES: usize = 2;

/// The half-open window a recap covers, in Unix milliseconds: `since_ms <= created < until_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecapWindow {
    /// The first instant inside the window.
    pub since_ms: i64,
    /// The first instant past it.
    pub until_ms: i64,
}

/// The per-channel read mark, as the header shows it. Secondary to the per-message Done marks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecapReadMark {
    /// The newest message the owner has been shown.
    pub last_read: MessageId,
    /// When the mark was set, in milliseconds since the Unix epoch.
    pub marked_at_ms: i64,
}

/// Message counts over the window, noise excluded from both sides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecapCounts {
    /// In-window messages the owner has marked Done.
    pub done: usize,
    /// In-window messages not marked Done.
    pub unread: usize,
    /// In-window messages the owner's noise rules call read; not listed.
    pub noise: usize,
}

/// One message with its read state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecapMessage {
    /// The message itself, as served.
    #[serde(flatten)]
    pub message: Message,
    /// Whether the owner has marked this message Done.
    pub done: bool,
}

/// A thread's opening, fetched because its root predates the window.
#[derive(Clone, Debug, Default)]
pub struct ThreadContext {
    /// The thread's root message.
    pub root: Option<Message>,
    /// The first replies after the root, at most [`CONTEXT_REPLIES`].
    pub opening: Vec<Message>,
    /// Replies before the window that are not shown (the opening excluded); `None` when the
    /// thread could not be walked to its start, so the count is unknown.
    pub hidden_before: Option<usize>,
    /// Whether the walk stopped before reaching the thread's start.
    pub truncated: bool,
}

/// One group of the window: a thread, or a single message that is in no thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecapGroup {
    /// The thread these messages belong to; absent for a message in no thread.
    pub thread_id: Option<String>,
    /// The thread's display name, when one has been chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// When the thread started (its root's creation instant, Unix ms); absent for no thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_started_ms: Option<i64>,
    /// Earlier replies hidden between the CONTEXT opening and the window; `None` when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_before: Option<usize>,
    /// Whether this thread's opening could not be read whole.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub context_truncated: bool,
    /// Pre-window opening: the root, then up to [`CONTEXT_REPLIES`] replies. Empty when the
    /// root is itself inside the window (then it is the first of `messages`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<RecapMessage>,
    /// The in-window messages, oldest first. For a thread: its root first when it is in-window,
    /// then its in-window replies.
    pub messages: Vec<RecapMessage>,
}

/// One channel's recap, as `GET /api/v1/channels/{id}/recap` answers it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecapResponse {
    /// The channel that was read.
    pub channel: ChannelInfo,
    /// The window that was read.
    pub window: RecapWindow,
    /// The channel's read mark, when this server holds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_mark: Option<RecapReadMark>,
    /// Done / unread / noise counts over the window.
    pub counts: RecapCounts,
    /// Whether the window walk hit its bound, so older in-window messages may be missing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The thread groups, ordered by their first in-window message.
    pub groups: Vec<RecapGroup>,
    /// Standing reminder that the content is third-party text.
    pub untrusted_content_notice: String,
}

/// Tag `message` with its read state.
fn tagged(message: Message, dismissed: &HashSet<MessageId>) -> RecapMessage {
    let done = dismissed.contains(&message.id);
    RecapMessage { message, done }
}

/// Group a window's messages (any order) by thread, oldest first.
///
/// `dismissed` is the channel's whole Done set; `contexts` holds the fetched opening of each
/// thread whose root predates the window. Pure, so the server and the tests share one grouping.
#[must_use]
pub fn group_window(
    mut messages: Vec<Message>,
    dismissed: &HashSet<MessageId>,
    contexts: &BTreeMap<String, ThreadContext>,
) -> Vec<RecapGroup> {
    messages.sort_by(|a, b| {
        a.created_ms()
            .unwrap_or(i64::MAX)
            .cmp(&b.created_ms().unwrap_or(i64::MAX))
            .then_with(|| a.id.numeric().cmp(&b.id.numeric()))
    });
    let mut order: Vec<Option<String>> = Vec::new();
    let mut by_thread: HashMap<Option<String>, Vec<Message>> = HashMap::new();
    for message in messages {
        let key = message.thread.as_ref().map(|thread| thread.id.clone());
        if !by_thread.contains_key(&key) {
            order.push(key.clone());
        }
        by_thread.entry(key).or_default().push(message);
    }
    // One group per standalone message, so threads and single messages interleave by time.
    let mut groups: Vec<RecapGroup> = Vec::new();
    for key in order {
        let held = by_thread.remove(&key).unwrap_or_default();
        match key {
            None => {
                for message in held {
                    groups.push(RecapGroup {
                        thread_id: None,
                        title: None,
                        thread_started_ms: None,
                        hidden_before: None,
                        context_truncated: false,
                        context: Vec::new(),
                        messages: vec![tagged(message, dismissed)],
                    });
                }
            }
            Some(thread_id) => {
                let root_in_window = held
                    .iter()
                    .find(|m| m.thread.as_ref().is_some_and(|thread| thread.is_root));
                let (context, hidden_before, context_truncated, started_ms, title) =
                    match (root_in_window, contexts.get(&thread_id)) {
                        (Some(root), _) => (
                            Vec::new(),
                            Some(0),
                            false,
                            root.created_ms(),
                            root.thread
                                .as_ref()
                                .and_then(|thread| thread.display_name.clone()),
                        ),
                        (None, Some(found)) => {
                            let mut context: Vec<RecapMessage> = Vec::new();
                            if let Some(root) = &found.root {
                                context.push(tagged(root.clone(), dismissed));
                            }
                            for reply in &found.opening {
                                context.push(tagged(reply.clone(), dismissed));
                            }
                            (
                                context,
                                found.hidden_before,
                                found.truncated,
                                found
                                    .root
                                    .as_ref()
                                    .and_then(|root| root.created_ms())
                                    .or_else(|| held.first().and_then(|m| m.created_ms())),
                                found.root.as_ref().and_then(|root| {
                                    root.thread
                                        .as_ref()
                                        .and_then(|thread| thread.display_name.clone())
                                }),
                            )
                        }
                        (None, None) => (
                            Vec::new(),
                            None,
                            false,
                            held.first().and_then(|m| m.created_ms()),
                            None,
                        ),
                    };
                groups.push(RecapGroup {
                    thread_id: Some(thread_id),
                    title,
                    thread_started_ms: started_ms,
                    hidden_before,
                    context_truncated,
                    context,
                    messages: held
                        .into_iter()
                        .map(|message| tagged(message, dismissed))
                        .collect(),
                });
            }
        }
    }
    groups.sort_by_key(|group| {
        group
            .messages
            .first()
            .and_then(|message| message.message.created_ms())
            .unwrap_or(i64::MAX)
    });
    groups
}

/// An instant as ET wall time: `2026-10-30 09:41 ET`.
#[must_use]
pub fn et_label(ms: i64) -> String {
    let zone = jiff::tz::TimeZone::get("America/New_York").unwrap_or(jiff::tz::TimeZone::UTC);
    jiff::Timestamp::from_millisecond(ms).map_or_else(
        |_| format!("{ms}"),
        |instant| format!("{} ET", instant.to_zoned(zone).strftime("%Y-%m-%d %H:%M")),
    )
}

/// A message's ET time label from its provider timestamp.
fn message_et(message: &Message) -> String {
    message.created_ms().map_or_else(
        || message.timestamp.clone(),
        |ms| {
            let zone =
                jiff::tz::TimeZone::get("America/New_York").unwrap_or(jiff::tz::TimeZone::UTC);
            jiff::Timestamp::from_millisecond(ms).map_or_else(
                |_| message.timestamp.clone(),
                |instant| format!("{} ET", instant.to_zoned(zone).strftime("%Y-%m-%d %H:%M")),
            )
        },
    )
}

/// The short id a message is cited by when it has no permalink: `#` and its last 8 characters.
fn short_id(id: &MessageId) -> String {
    let text = id.as_str();
    let tail: String = text
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("#{tail}")
}

/// How much of a DONE message the markdown shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DoneMode {
    /// The full text.
    Full,
    /// Its first line, cut at 160 characters.
    OneLine,
    /// Nothing past the header line.
    HeaderOnly,
}

fn message_line(message: &RecapMessage, mode: DoneMode, context: bool) -> String {
    let held = &message.message;
    let link = held
        .upstream_url
        .as_deref()
        .map_or_else(|| short_id(&held.id), |url| format!("[↗]({url})"));
    let state = if context {
        "CONTEXT"
    } else if message.done {
        "DONE"
    } else {
        "UNREAD"
    };
    let head = format!(
        "- {} · {} · {} · {}",
        message_et(held),
        held.author,
        state,
        link
    );
    let body = held.content.trim();
    if body.is_empty() || (message.done && !context && mode == DoneMode::HeaderOnly) {
        return head;
    }
    if (message.done && mode == DoneMode::OneLine) || context {
        let first = body.lines().next().unwrap_or("");
        let cut: String = first.chars().take(160).collect();
        let ellipsis = if first.chars().count() > 160 || body.lines().count() > 1 {
            " …"
        } else {
            ""
        };
        return format!("{head} — {cut}{ellipsis}");
    }
    let indented = body.replace('\n', "\n  ");
    format!("{head}\n  {indented}")
}

/// Render one channel's recap as markdown at a DONE verbosity.
fn render_channel(response: &RecapResponse, mode: DoneMode) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Recap — {} ({})\n\n",
        response.channel.display_name(),
        response.channel.id
    ));
    let mark = response.read_mark.as_ref().map_or_else(
        || "no read mark held".to_owned(),
        |mark| {
            format!(
                "read mark {} (through {})",
                et_label(mark.marked_at_ms),
                short_id(&mark.last_read)
            )
        },
    );
    out.push_str(&format!(
        "Window: {} to {} · {} · DONE {} · UNREAD {} · {} noise hidden\n",
        et_label(response.window.since_ms),
        et_label(response.window.until_ms),
        mark,
        response.counts.done,
        response.counts.unread,
        response.counts.noise
    ));
    if response.truncated {
        out.push_str(
            "Note: the window walk hit its bound; older in-window messages may be missing.\n",
        );
    }
    out.push('\n');
    for group in &response.groups {
        match &group.thread_id {
            None => {
                for message in &group.messages {
                    out.push_str(&message_line(message, mode, false));
                    out.push('\n');
                }
            }
            Some(thread_id) => {
                let name = group.title.as_deref().unwrap_or("Thread");
                out.push_str(&format!("\n## {name} ({thread_id})\n"));
                if !group.context.is_empty() {
                    if let Some(started) = group.thread_started_ms {
                        let hidden = match group.hidden_before {
                            Some(count) => format!("{count} earlier replies hidden"),
                            None => "earlier replies hidden (count unknown)".to_owned(),
                        };
                        out.push_str(&format!(
                            "Thread started {} · {hidden}\n",
                            et_label(started)
                        ));
                    }
                    for message in &group.context {
                        out.push_str(&message_line(message, mode, true));
                        out.push('\n');
                    }
                } else if group.context_truncated {
                    out.push_str("Thread opening could not be read whole.\n");
                }
                for message in &group.messages {
                    out.push_str(&message_line(message, mode, false));
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Render every channel's recap as one markdown document within `max_chars`.
///
/// Over budget, DONE messages shrink to one line first, then to their header line; UNREAD text
/// is never cut, so the result may still exceed the budget — and then says so. `max_chars` of 0
/// means no limit.
#[must_use]
pub fn render(channels: &[RecapResponse], max_chars: usize) -> String {
    let full = channels
        .iter()
        .map(|response| render_channel(response, DoneMode::Full))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
        + FOOTER
        + "\n";
    if max_chars == 0 || full.chars().count() <= max_chars {
        return full;
    }
    let one_line = channels
        .iter()
        .map(|response| render_channel(response, DoneMode::OneLine))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
        + FOOTER
        + "\n";
    if one_line.chars().count() <= max_chars {
        return one_line;
    }
    let headers = channels
        .iter()
        .map(|response| render_channel(response, DoneMode::HeaderOnly))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
        + FOOTER
        + "\n";
    if headers.chars().count() <= max_chars {
        return headers;
    }
    headers + "\n[Over the character budget on UNREAD messages alone; nothing UNREAD was cut.]\n"
}

/// What `recap` was asked to do.
#[derive(Debug)]
pub struct Options {
    /// `--config`.
    pub config: Option<PathBuf>,
    /// `--config-overlay`, in order.
    pub overlays: Vec<PathBuf>,
    /// `--server`.
    pub server: Option<String>,
    /// `--hours`.
    pub hours: Option<u64>,
    /// `--since`, as written.
    pub since: Option<String>,
    /// `--channel`, as written.
    pub channel: Option<String>,
    /// `--max-chars`.
    pub max_chars: usize,
    /// `--json`.
    pub json: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            config: None,
            overlays: Vec::new(),
            server: None,
            hours: None,
            since: None,
            channel: None,
            max_chars: 24_000,
            json: false,
        }
    }
}

/// Parse the arguments after `recap`. `Ok(None)` means `--help` was printed.
///
/// # Errors
///
/// An unknown argument, a flag without its value, `--hours` with `--since`, or a number that
/// does not parse.
pub fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Option<Options>> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "--config" => {
                options.config = Some(args.next().context("--config needs a path")?.into())
            }
            "--config-overlay" => options
                .overlays
                .push(args.next().context("--config-overlay needs a path")?.into()),
            "--server" => options.server = Some(args.next().context("--server needs a URL")?),
            "--hours" => {
                let value = args.next().context("--hours needs a number")?;
                options.hours = Some(
                    value
                        .parse::<u64>()
                        .with_context(|| format!("--hours takes whole hours, not {value:?}"))?,
                );
            }
            "--since" => options.since = Some(args.next().context("--since needs a value")?),
            "--channel" => options.channel = Some(args.next().context("--channel needs a value")?),
            "--max-chars" => {
                let value = args.next().context("--max-chars needs a number")?;
                options.max_chars = value.parse::<usize>().with_context(|| {
                    format!("--max-chars takes a character count, not {value:?}")
                })?;
            }
            "--json" => options.json = true,
            other => anyhow::bail!("unrecognized argument {other:?}\n\n{USAGE}"),
        }
    }
    if options.hours.is_some() && options.since.is_some() {
        anyhow::bail!("--hours and --since name two windows; give one\n\n{USAGE}");
    }
    if let Some(hours) = options.hours {
        if !(1..=168).contains(&hours) {
            anyhow::bail!("--hours must be between 1 and 168, not {hours}");
        }
    }
    Ok(Some(options))
}

/// An ET (`America/New_York`) zone for `--since` wall times.
fn eastern() -> anyhow::Result<jiff::tz::TimeZone> {
    jiff::tz::TimeZone::get("America/New_York").context("loading the America/New_York zone")
}

/// Parse a `--since` wall time in ET: `YYYY-MM-DD` or `YYYY-MM-DD HH:MM`.
fn et_wall_ms(raw: &str) -> Option<i64> {
    let (date, time) = match raw.split_once([' ', 'T']) {
        Some((date, time)) => (date, Some(time)),
        None => (raw, None),
    };
    let mut date_parts = date.split('-');
    let year: i32 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let (hour, minute): (u32, u32) = match time {
        None => (0, 0),
        Some(time) => {
            let mut parts = time.split(':');
            let hour: u32 = parts.next()?.parse().ok()?;
            let minute: u32 = parts.next()?.parse().ok()?;
            (hour, minute)
        }
    };
    let date = jiff::civil::Date::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
    )
    .ok()?;
    let time =
        jiff::civil::Time::new(i8::try_from(hour).ok()?, i8::try_from(minute).ok()?, 0, 0).ok()?;
    date.to_datetime(time)
        .to_zoned(eastern().ok()?)
        .ok()
        .map(|zoned| zoned.timestamp().as_millisecond())
}

/// What `--since` resolved to before any server lookup: an instant, or a message id to look up.
enum Since {
    /// Window start, Unix ms.
    At(i64),
    /// A message id whose instant the server must supply.
    Message(String),
    /// A permalink's channel hint plus its message id.
    Permalink(Option<String>, String),
}

/// Resolve `--since` as far as this process can without the server.
///
/// # Errors
///
/// The value is none of a permalink, a snowflake, an instant, or an ET wall time — it can
/// still be a provider message id, which [`Since::Message`] carries to the lookup.
fn resolve_since(raw: &str) -> Since {
    let raw = raw.trim();
    if let Some(target) = crate::message_links::recognize(raw) {
        if let Some(message) = target.message {
            return Since::Permalink(Some(target.channel), message);
        }
    }
    if raw.chars().all(|c| c.is_ascii_digit()) && raw.len() >= 15 {
        if let Some(ms) = MessageId(raw.to_owned()).created_at_ms() {
            return Since::At(ms);
        }
    }
    if let Ok(instant) = raw.parse::<jiff::Timestamp>() {
        return Since::At(instant.as_millisecond());
    }
    if let Some(ms) = et_wall_ms(raw) {
        return Since::At(ms);
    }
    Since::Message(raw.to_owned())
}

/// The server's address: `--server`, else the configuration's bind address on this host.
fn server_base(options: &Options, config: &Config) -> anyhow::Result<String> {
    if let Some(server) = &options.server {
        let parsed =
            url::Url::parse(server).with_context(|| format!("--server {server:?} is not a URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            anyhow::bail!("--server {server:?} must be an http:// or https:// address");
        }
        return Ok(server.trim_end_matches('/').to_owned());
    }
    let local: SocketAddr = crate::pair_link::local_address(config.bind);
    if !crate::pair_link::is_this_host(local.ip()) {
        anyhow::bail!(
            "the configuration's bind address {} is not an address of this host, and the read \
             token is sent only to this host; run recap where the server runs, or pass --server",
            local.ip()
        );
    }
    Ok(format!("http://{local}"))
}

/// One failed HTTP answer, in the server's own words where it gave them.
async fn read_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> anyhow::Result<T> {
    let status = response.status();
    let body = response.bytes().await.context("reading the answer")?;
    if !status.is_success() {
        #[derive(serde::Deserialize)]
        struct Refusal {
            error: String,
            detail: String,
        }
        let detail = serde_json::from_slice::<Refusal>(&body)
            .map(|error| format!("{}: {}", error.error, error.detail))
            .unwrap_or_else(|_| "no detail".to_owned());
        anyhow::bail!("{what} failed ({status}): {detail}");
    }
    serde_json::from_slice(&body).with_context(|| format!("{what} answered unexpectedly"))
}

/// Run `vibe-talk recap` and return what to print.
///
/// # Errors
///
/// The configuration cannot be loaded, the window is unusable, the server cannot be reached,
/// or it refuses.
pub async fn run(options: Options) -> anyhow::Result<String> {
    let config = Config::discover(options.config.clone(), options.overlays.clone()).context(
        "recap reads the server's configuration for its read token and bind address, as the \
         server does: run it with the server's --config and environment",
    )?;
    let base = server_base(&options, &config)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()
        .context("building the HTTP client")?;
    let token = config.auth.read_token.expose().to_owned();
    let channels: Vec<ChannelInfo> = read_json(
        client
            .get(format!("{base}/api/v1/channels"))
            .bearer_auth(&token)
            .send()
            .await
            .context("could not reach the vibe-talk server; is it running?")?,
        "listing channels",
    )
    .await
    .and_then(|body: serde_json::Value| {
        serde_json::from_value(body["channels"].clone()).context("listing channels")
    })?;
    if channels.is_empty() {
        anyhow::bail!("the server lists no channels for this read token");
    }
    // Resolve --since now that the channel list can answer a message lookup.
    let until_ms = crate::ops::now_ms();
    let (since_ms, hint) = match &options.since {
        None => {
            let hours = options.hours.unwrap_or(24);
            (
                until_ms - i64::try_from(hours).unwrap_or(24) * 3_600_000,
                None,
            )
        }
        Some(raw) => match resolve_since(raw) {
            Since::At(ms) => (ms, None),
            Since::Permalink(channel, message) => {
                let ms = message_ms(
                    &client,
                    &base,
                    &token,
                    &channels,
                    channel.as_deref(),
                    &message,
                )
                .await?;
                (ms, channel)
            }
            Since::Message(message) => (
                message_ms(&client, &base, &token, &channels, None, &message).await?,
                None,
            ),
        },
    };
    if since_ms >= until_ms {
        anyhow::bail!("the window is empty: --since is not before now");
    }
    let wanted = options.channel.as_deref().or(hint.as_deref());
    let selected: Vec<&ChannelInfo> = channels
        .iter()
        .filter(|channel| {
            wanted.is_none_or(|want| {
                channel.id.as_str().eq_ignore_ascii_case(want)
                    || channel.display_name().eq_ignore_ascii_case(want)
                    || channel.label.eq_ignore_ascii_case(want)
            })
        })
        .collect();
    if selected.is_empty() {
        anyhow::bail!(
            "no channel matches {:?}; the read token sees: {}",
            wanted.unwrap_or_default(),
            channels
                .iter()
                .map(|channel| format!("{} ({})", channel.display_name(), channel.id))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let mut responses: Vec<RecapResponse> = Vec::new();
    for channel in selected {
        let response: RecapResponse = read_json(
            client
                .get(format!("{base}/api/v1/channels/{}/recap", channel.id))
                .query(&[("since_ms", since_ms), ("until_ms", until_ms)])
                .bearer_auth(&token)
                .send()
                .await
                .with_context(|| format!("reading the recap of {}", channel.display_name()))?,
            "the recap",
        )
        .await?;
        responses.push(response);
    }
    if options.json {
        return serde_json::to_string_pretty(&serde_json::json!({ "channels": responses }))
            .context("rendering JSON");
    }
    Ok(render(&responses, options.max_chars))
}

/// The creation instant of message `message_id`, from its snowflake or from the server.
async fn message_ms(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    channels: &[ChannelInfo],
    hint: Option<&str>,
    message_id: &str,
) -> anyhow::Result<i64> {
    if let Some(ms) = MessageId(message_id.to_owned()).created_at_ms() {
        return Ok(ms);
    }
    let ordered: Vec<&ChannelInfo> = match hint {
        Some(hint) => channels
            .iter()
            .filter(|channel| channel.id.as_str() == hint)
            .chain(
                channels
                    .iter()
                    .filter(|channel| channel.id.as_str() != hint),
            )
            .collect(),
        None => channels.iter().collect(),
    };
    for channel in ordered {
        let url = format!(
            "{base}/api/v1/channels/{}/messages/{}",
            channel.id,
            urlencoding(message_id)
        );
        let response = client
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .context("looking the message up")?;
        if response.status().is_success() {
            let body: serde_json::Value = read_json(response, "the message lookup").await?;
            let timestamp = body["message"]["timestamp"]
                .as_str()
                .context("the message lookup answered without a timestamp")?;
            return crate::clock::instant_ms(timestamp).with_context(|| {
                format!("the message's timestamp {timestamp:?} is not an instant")
            });
        }
    }
    anyhow::bail!(
        "no channel the read token sees holds message {message_id:?}; give --since as a \
         permalink, a snowflake id, or a time"
    )
}

/// Percent-encode one URL path segment.
fn urlencoding(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChannelId, UserId};
    use crate::threads::MessageThread;

    fn at(ms: i64, seq: u64) -> MessageId {
        MessageId((MessageId::at_time_ms(ms).numeric().expect("snowflake") + seq).to_string())
    }

    fn held(ms: i64, seq: u64, content: &str, thread: Option<(&str, bool)>) -> Message {
        Message {
            edited_timestamp: None,
            id: at(ms, seq),
            channel_id: ChannelId("2222222222".to_owned()),
            author: "agent".to_owned(),
            author_id: UserId("7".to_owned()),
            author_is_bot: false,
            timestamp: crate::clock::iso_from_ms(ms),
            spoken_time: String::new(),
            reply_to: None,
            content: content.to_owned(),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            thread: thread.map(|(id, is_root)| MessageThread {
                hue: None,
                display_name: None,
                summary: None,
                id: id.to_owned(),
                root_message_id: None,
                is_root,
                broadcast: false,
                reply_count: None,
                reply_count_exact: false,
            }),
            reactions: None,
            heading: None,
            mentions: Vec::new(),
            upstream_url: None,
            upstream_app_url: None,
            op_key: None,
            observed_ms: None,
        }
    }

    const BASE: i64 = 1_787_000_000_000;

    #[test]
    fn grouping_keeps_a_thread_together_and_a_standalone_apart() {
        let root = held(BASE, 1, "root", Some(("t1", true)));
        let reply = held(BASE + 60_000, 2, "reply", Some(("t1", false)));
        let alone = held(BASE + 30_000, 3, "alone", None);
        let dismissed: HashSet<MessageId> = [root.id.clone()].into_iter().collect();
        let groups = group_window(
            vec![reply.clone(), alone, root.clone()],
            &dismissed,
            &BTreeMap::new(),
        );
        assert_eq!(groups.len(), 2);
        // The thread's first window message (its root, at BASE) precedes the standalone (+30s).
        assert_eq!(groups[0].thread_id.as_deref(), Some("t1"));
        assert_eq!(groups[1].thread_id, None);
        assert_eq!(groups[0].messages.len(), 2);
        assert!(groups[0].messages[0].done, "the dismissed root");
        assert!(!groups[0].messages[1].done, "its reply is not Done with it");
        assert_eq!(groups[0].hidden_before, Some(0));
        assert!(groups[0].context.is_empty());
    }

    #[test]
    fn grouping_uses_the_fetched_opening_for_a_pre_window_thread() {
        let reply = held(BASE + 60_000, 9, "in window", Some(("t1", false)));
        let root = held(BASE - 3_600_000, 1, "old root", Some(("t1", true)));
        let opening = held(BASE - 3_500_000, 2, "first reply", Some(("t1", false)));
        let mut contexts = BTreeMap::new();
        contexts.insert(
            "t1".to_owned(),
            ThreadContext {
                root: Some(root),
                opening: vec![opening],
                hidden_before: Some(4),
                truncated: false,
            },
        );
        let groups = group_window(vec![reply], &HashSet::new(), &contexts);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].context.len(), 2);
        assert_eq!(groups[0].hidden_before, Some(4));
        assert_eq!(groups[0].messages.len(), 1);
    }

    fn response_with(done_text: &str, unread_text: &str) -> RecapResponse {
        let done = held(BASE, 1, done_text, None);
        let unread = held(BASE + 60_000, 2, unread_text, None);
        RecapResponse {
            channel: ChannelInfo {
                id: ChannelId("2222222222".to_owned()),
                label: "lead team".to_owned(),
                writable: true,
                alias: None,
                added: false,
                provider: None,
                write_source: None,
            },
            window: RecapWindow {
                since_ms: BASE,
                until_ms: BASE + 3_600_000,
            },
            read_mark: None,
            counts: RecapCounts {
                done: 1,
                unread: 1,
                noise: 0,
            },
            truncated: false,
            groups: group_window(
                vec![done, unread],
                &HashSet::from([at(BASE, 1)]),
                &BTreeMap::new(),
            ),
            untrusted_content_notice: "notice".to_owned(),
        }
    }

    #[test]
    fn over_budget_the_done_shrink_and_the_unread_never_do() {
        let response = response_with(
            "done first line\ndone second line that must vanish over budget",
            "unread text that must survive every shrink level however long it runs",
        );
        let shrunk = render(std::slice::from_ref(&response), 320);
        assert!(shrunk.contains("unread text that must survive"), "{shrunk}");
        assert!(!shrunk.contains("done second line"), "{shrunk}");
        assert!(shrunk.contains("DONE"), "{shrunk}");
        assert!(shrunk.contains("UNREAD"), "{shrunk}");
        assert!(shrunk.contains("do not repeat DONE"), "{shrunk}");
    }

    #[test]
    fn et_labels_follow_daylight_saving() {
        let winter = crate::clock::instant_ms("2026-01-15T14:00:00Z").expect("instant");
        let summer = crate::clock::instant_ms("2026-07-15T14:00:00Z").expect("instant");
        assert_eq!(et_label(winter), "2026-01-15 09:00 ET");
        assert_eq!(et_label(summer), "2026-07-15 10:00 ET");
    }

    #[test]
    fn a_since_snowflake_and_wall_time_resolve_without_the_server() {
        let id = MessageId::at_time_ms(BASE);
        match resolve_since(id.as_str()) {
            Since::At(ms) => assert_eq!(ms, BASE),
            _ => panic!("a snowflake should resolve at once"),
        }
        match resolve_since("2026-10-30") {
            Since::At(ms) => assert_eq!(ms, et_wall_ms("2026-10-30").expect("midnight ET")),
            _ => panic!("a date should resolve at once"),
        }
        match resolve_since("2026-10-30T14:00:00Z") {
            Since::At(ms) => {
                assert_eq!(
                    ms,
                    crate::clock::instant_ms("2026-10-30T14:00:00Z").expect("ms")
                )
            }
            _ => panic!("an RFC 3339 instant should resolve at once"),
        }
    }

    #[test]
    fn the_arguments_refuse_two_windows() {
        assert!(parse(["--hours", "2", "--since", "2026-10-30"].map(String::from)).is_err());
        assert!(parse(["--hours", "999"].map(String::from)).is_err());
        assert!(parse(["--nope"].map(String::from)).is_err());
        let options = parse(["--hours", "12", "--json"].map(String::from))
            .expect("parse")
            .expect("options");
        assert_eq!(options.hours, Some(12));
        assert!(options.json);
    }
}
