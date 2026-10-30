//! A bridge's event sinks: a small JSON record of every message it posts and every reaction it
//! adds, delivered to configured HTTP endpoints. `#708 vt-live-push`.
//!
//! # Why
//!
//! A reader of the conversation through another program (a web client keeping its own copy, a
//! search index) learns of the bridge's own posts only by reading the provider, and a reader whose
//! provider reads are rate-limited learns of them minutes late. A provider's event stream, tailed
//! as a person, leaves other bots' posts out. The bridge knows the instant it posted: it tells
//! each sink, which applies the record as the provider's own event would be applied, and keeps
//! reading the provider only to reconcile.
//!
//! # Configuration
//!
//! `event-sinks.json` in the bridge's state directory, private to its owner. Absent means no
//! sinks. Read when a transport is opened, so a running bridge takes a change at its next start.
//!
//! ```json
//! {
//!   "version": 1,
//!   "sender": {"id": "U0BRIDGE01", "name": "bridge", "bot": true},
//!   "sinks": [{
//!     "name": "reader",
//!     "url": "https://reader.example:8422/api/v1/live/tail",
//!     "token_file": "event-sink-reader.token",
//!     "kinds": ["message.posted", "reaction.added"]
//!   }]
//! }
//! ```
//!
//! `sender` is the bridge's own identity at the provider, as a reader of the provider would name
//! the author of its posts; a sink may need it to file a post under the same author a later read
//! will name. `token_file` is relative to the state directory and must be private (mode 0600): its
//! one line is sent as `Authorization: Bearer`, never in an argument a process listing shows.
//! `kinds` chooses the records: `message.posted` (every post: replies, top-level fallbacks, root
//! messages) and `reaction.added` (every reaction the bridge ensured, its ✅ and ❌ as much as its
//! 👀, also when the provider found it already there, since an earlier attempt may have added it
//! and lost the answer; a sink takes it as present, not as one more). Optional per sink:
//! `max_queued` (records kept while the sink cannot be reached; default 1000, oldest dropped and
//! counted), `max_age_seconds` (older records are dropped and counted; default 900, which is as
//! long as a reader applying pushes usually takes one as news), and `curl` (`executable` and extra
//! `arguments`, for a CA bundle or a proxy; default `curl` on `PATH`).
//!
//! # The record
//!
//! ```json
//! {"schema": "agentctl.chat-event.v1", "event_id": "message.posted:<uuid>",
//!  "kind": "message.posted", "at": "2026-10-30T16:25:17.953Z", "bridge": "coordinator",
//!  "channel_id": "C0CHANNEL1", "message_id": "C0CHANNEL1/messages/1793377517.953000",
//!  "thread_id": "C0CHANNEL1/threads/1793377304.077519", "text": "…", "text_format": "markdown",
//!  "sender": {"id": "U0BRIDGE01", "name": "bridge", "bot": true}
//! }
//! ```
//!
//! A reaction record carries `emoji` (the configured reaction) in place of `text` and
//! `thread_id`. `event_id` is stable across the bridge's own retries of the operation, so a sink
//! deduplicates by it.
//!
//! # Delivery
//!
//! Recording never fails or delays the operation it records: the record is written to
//! `event-sinks/<sink>/queue/` (one file each, oldest first) and the running bridge's delivery
//! worker posts it with `curl`, one at a time per sink, in order. A 2xx answer delivers it; a
//! 400, 413, 415 or 422 refuses it for good, and it is dropped and counted; anything else (no
//! answer, 401, 403, 404, 429, 5xx) is retried with a growing pause, up to five minutes. What is
//! dropped is counted in `event-sinks/<sink>/dropped.json`, so a reader of the sink can tell a gap
//! from a quiet bridge.

use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

/// The configuration file, in the bridge's state directory.
pub const SINKS_FILE: &str = "event-sinks.json";
/// The directory of every sink's queue and counts, in the bridge's state directory.
pub const SINKS_DIRECTORY: &str = "event-sinks";
/// The record's schema name.
pub const SCHEMA: &str = "agentctl.chat-event.v1";

const DEFAULT_MAX_QUEUED: usize = 1000;
const DEFAULT_MAX_AGE_SECONDS: u64 = 900;
const MAX_TOKEN_BYTES: u64 = 4096;
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const CURL_MAX_TIME_SECONDS: u64 = 15;
/// How often the worker looks for records another process queued.
const POLL: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Which records a sink takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Kind {
    /// Every message the bridge posted.
    #[serde(rename = "message.posted")]
    MessagePosted,
    /// Every reaction the bridge ensured.
    #[serde(rename = "reaction.added")]
    ReactionAdded,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::MessagePosted => "message.posted",
            Self::ReactionAdded => "reaction.added",
        }
    }
}

/// The bridge's own identity at the provider, as the configuration names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sender {
    /// The provider's id of the account the bridge posts as.
    #[serde(default)]
    pub id: Option<String>,
    /// The name a reader shows for it.
    #[serde(default)]
    pub name: Option<String>,
    /// Whether that account is a bot.
    #[serde(default = "yes")]
    pub bot: bool,
}

fn yes() -> bool {
    true
}

/// How a sink is reached with `curl`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Curl {
    /// The `curl` executable; `curl` on `PATH` when absent.
    #[serde(default)]
    pub executable: Option<PathBuf>,
    /// Extra arguments, before the ones the worker gives.
    #[serde(default)]
    pub arguments: Vec<String>,
}

/// One configured sink.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SinkConfig {
    /// Lowercase letters, digits and hyphens: the name of its directory.
    pub name: String,
    /// Where each record is posted.
    pub url: String,
    /// The bearer token's file, relative to the state directory.
    pub token_file: PathBuf,
    /// The records it takes.
    pub kinds: Vec<Kind>,
    /// Records kept while it cannot be reached.
    #[serde(default = "default_max_queued")]
    pub max_queued: usize,
    /// How old a record may grow before it is dropped, in seconds.
    #[serde(default = "default_max_age_seconds")]
    pub max_age_seconds: u64,
    /// How it is reached.
    #[serde(default)]
    pub curl: Curl,
}

fn default_max_queued() -> usize {
    DEFAULT_MAX_QUEUED
}

fn default_max_age_seconds() -> u64 {
    DEFAULT_MAX_AGE_SECONDS
}

/// `event-sinks.json`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SinksFile {
    /// Always 1.
    pub version: u32,
    /// The bridge's own identity at the provider.
    #[serde(default)]
    pub sender: Sender,
    /// The sinks.
    #[serde(default)]
    pub sinks: Vec<SinkConfig>,
}

impl SinksFile {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("version {} is not 1", self.version));
        }
        let mut names = std::collections::BTreeSet::new();
        for sink in &self.sinks {
            let slug = !sink.name.is_empty()
                && sink.name.len() <= 64
                && sink
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !slug {
                return Err(format!(
                    "sink name {:?} is not lowercase letters, digits and hyphens",
                    sink.name
                ));
            }
            if !names.insert(sink.name.as_str()) {
                return Err(format!("sink name {:?} is used twice", sink.name));
            }
            if !(sink.url.starts_with("https://") || sink.url.starts_with("http://"))
                || sink
                    .url
                    .chars()
                    .any(|c| c.is_control() || c == '"' || c == '\\')
            {
                return Err(format!("sink {:?}: url must be an http(s) URL", sink.name));
            }
            if sink.token_file.is_absolute()
                || sink
                    .token_file
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                return Err(format!(
                    "sink {:?}: token_file must be a plain path inside the state directory",
                    sink.name
                ));
            }
            if sink.kinds.is_empty() {
                return Err(format!("sink {:?}: kinds is empty", sink.name));
            }
            if sink.max_queued == 0 {
                return Err(format!(
                    "sink {:?}: max_queued must be at least 1",
                    sink.name
                ));
            }
        }
        Ok(())
    }
}

/// A bridge's sinks, as its state directory configures them.
#[derive(Debug)]
pub struct EventSink {
    root: PathBuf,
    bridge: String,
    config: SinksFile,
    counter: AtomicU64,
}

/// The pid and a per-process counter keep two writers' file names apart.
fn unique_suffix(counter: &AtomicU64) -> String {
    format!(
        "{:010}-{:020}",
        std::process::id(),
        counter.fetch_add(1, Ordering::Relaxed)
    )
}

impl EventSink {
    /// The sinks `root`'s `event-sinks.json` configures for the bridge `bridge`, or `None` when it
    /// configures none. A file that cannot be read or is malformed is reported on stderr and
    /// configures none: recording must never stop the bridge.
    pub fn load(root: &Path, bridge: &str) -> Option<Arc<Self>> {
        let path = root.join(SINKS_FILE);
        let config = match read_config(&path) {
            Ok(Some(config)) => config,
            Ok(None) => return None,
            Err(error) => {
                eprintln!(
                    "agentctl: chat event sinks are off: {} could not be used: {error}",
                    path.display()
                );
                return None;
            }
        };
        if config.sinks.is_empty() {
            return None;
        }
        Some(Arc::new(Self {
            root: root.to_path_buf(),
            bridge: bridge.to_owned(),
            config,
            counter: AtomicU64::new(0),
        }))
    }

    /// The configuration in use.
    #[must_use]
    pub fn config(&self) -> &SinksFile {
        &self.config
    }

    fn sink_dir(&self, sink: &SinkConfig) -> PathBuf {
        self.root.join(SINKS_DIRECTORY).join(&sink.name)
    }

    fn sender_json(&self) -> Value {
        let sender = &self.config.sender;
        json!({"id": sender.id, "name": sender.name, "bot": sender.bot})
    }

    /// Record that the bridge posted `message_id` in `channel_id` (in `thread_id`, when it
    /// answered in a thread) with `text`, under the operation's stable `request_id`.
    pub fn record_posted(
        &self,
        channel_id: &str,
        message_id: &str,
        thread_id: Option<&str>,
        text: &str,
        request_id: &str,
    ) {
        let mut record = self.record(Kind::MessagePosted, request_id, channel_id, message_id);
        record["thread_id"] = json!(thread_id);
        record["text"] = json!(text);
        record["text_format"] = json!("markdown");
        record["sender"] = self.sender_json();
        self.enqueue(Kind::MessagePosted, &record);
    }

    /// Record that the bridge added `emoji` to `message_id` in `channel_id`.
    pub fn record_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
        request_id: &str,
    ) {
        let mut record = self.record(Kind::ReactionAdded, request_id, channel_id, message_id);
        record["emoji"] = json!(emoji);
        record["sender"] = self.sender_json();
        self.enqueue(Kind::ReactionAdded, &record);
    }

    fn record(&self, kind: Kind, operation: &str, channel_id: &str, message_id: &str) -> Value {
        json!({
            "schema": SCHEMA,
            "event_id": format!("{}:{operation}", kind.as_str()),
            "kind": kind.as_str(),
            "at": rfc3339_millis(crate::chat_runtime::unix_millis()),
            "bridge": self.bridge,
            "channel_id": channel_id,
            "message_id": message_id,
        })
    }

    fn enqueue(&self, kind: Kind, record: &Value) {
        for sink in self
            .config
            .sinks
            .iter()
            .filter(|sink| sink.kinds.contains(&kind))
        {
            if let Err(error) = self.enqueue_one(sink, record) {
                eprintln!(
                    "agentctl: chat event sink {:?} could not queue a {} record: {error}",
                    sink.name,
                    kind.as_str()
                );
            }
        }
    }

    fn enqueue_one(&self, sink: &SinkConfig, record: &Value) -> io::Result<()> {
        let queue = self.sink_dir(sink).join("queue");
        private_dirs(&queue)?;
        let name = format!(
            "{:020}-{}.json",
            crate::chat_runtime::unix_millis(),
            unique_suffix(&self.counter)
        );
        let temporary = queue.join(format!(".{name}.tmp"));
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode_private()
                .open(&temporary)?;
            file.write_all(record.to_string().as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&temporary, queue.join(&name))?;
        let queued = queued(&queue)?;
        let excess = queued.len().saturating_sub(sink.max_queued);
        for oldest in queued.iter().take(excess) {
            if fs::remove_file(oldest).is_ok() {
                self.count_dropped(sink, "the queue was full");
            }
        }
        Ok(())
    }

    fn count_dropped(&self, sink: &SinkConfig, why: &str) {
        let path = self.sink_dir(sink).join("dropped.json");
        let held = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let count = held
            .as_ref()
            .and_then(|value| value.get("dropped"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        let value = json!({
            "dropped": count,
            "last_dropped_at": rfc3339_millis(crate::chat_runtime::unix_millis()),
            "last_reason": why,
        });
        if let Err(error) = crate::agent::atomic_json(&path, &value) {
            eprintln!(
                "agentctl: chat event sink {:?} could not count a dropped record: {error}",
                sink.name
            );
        }
    }

    /// Post each due sink's oldest queued record, and as many after it as are delivered or
    /// refused for good, until one waits for a retry. `retry` holds each sink's pause and when it
    /// is next due, so a sink that cannot be reached holds back no other. Returns how long until
    /// the next sink is due, at most [`POLL`].
    pub fn deliver(&self, retry: &mut [Retry], log: &dyn Fn(String)) -> Duration {
        let now = std::time::Instant::now();
        for (index, sink) in self.config.sinks.iter().enumerate() {
            let state = &mut retry[index];
            if state.due.is_some_and(|due| due > now) {
                continue;
            }
            let queue = self.sink_dir(sink).join("queue");
            let Ok(queued) = queued(&queue) else {
                continue;
            };
            for path in queued {
                if record_age(&path)
                    .is_some_and(|age| age > Duration::from_secs(sink.max_age_seconds))
                {
                    let _ = fs::remove_file(&path);
                    self.count_dropped(sink, "it grew older than max_age_seconds");
                    continue;
                }
                match self.post(sink, &path) {
                    Posted::Delivered => {
                        let _ = fs::remove_file(&path);
                        *state = Retry::default();
                    }
                    Posted::Refused(status) => {
                        let _ = fs::remove_file(&path);
                        self.count_dropped(sink, "the sink refused it");
                        log(format!(
                            "agentctl: chat event sink {:?} refused a record for good (HTTP {status}); dropped",
                            sink.name
                        ));
                        *state = Retry::default();
                    }
                    Posted::Retry(why) => {
                        if state.pause.is_zero() {
                            log(format!(
                                "agentctl: chat event sink {:?} could not take a record ({why}); retrying",
                                sink.name
                            ));
                        }
                        state.pause = if state.pause.is_zero() {
                            Duration::from_secs(1)
                        } else {
                            (state.pause * 2).min(MAX_BACKOFF)
                        };
                        state.due = Some(std::time::Instant::now() + state.pause);
                        break;
                    }
                }
            }
        }
        let now = std::time::Instant::now();
        retry
            .iter()
            .filter_map(|state| state.due)
            .map(|due| due.saturating_duration_since(now))
            .fold(POLL, Duration::min)
    }

    fn post(&self, sink: &SinkConfig, path: &Path) -> Posted {
        let token = match read_token(&self.root.join(&sink.token_file)) {
            Ok(token) => token,
            Err(error) => return Posted::Retry(format!("the token file: {error}")),
        };
        let executable = sink
            .curl
            .executable
            .clone()
            .unwrap_or_else(|| PathBuf::from("curl"));
        // The URL, the token and the record's path go to curl on its standard input as its
        // configuration, never as arguments a process listing shows.
        let config = format!(
            "url = \"{}\"\nheader = \"Authorization: Bearer {token}\"\nheader = \"Content-Type: application/json\"\ndata-binary = \"@{}\"\noutput = \"/dev/null\"\nwrite-out = \"%{{http_code}}\"\n",
            sink.url,
            curl_quote(&path.to_string_lossy()),
        );
        let child = Command::new(&executable)
            .args(&sink.curl.arguments)
            .args([
                "--silent",
                "--show-error",
                "--max-time",
                &CURL_MAX_TIME_SECONDS.to_string(),
                "--config",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                return Posted::Retry(format!("{} did not start: {error}", executable.display()))
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(config.as_bytes()) {
                let _ = child.kill();
                let _ = child.wait();
                return Posted::Retry(format!(
                    "curl's configuration could not be written: {error}"
                ));
            }
        }
        let output = match child.wait_with_output() {
            Ok(output) => output,
            Err(error) => return Posted::Retry(format!("curl did not finish: {error}")),
        };
        let status: u16 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        match status {
            200..=299 => Posted::Delivered,
            400 | 413 | 415 | 422 => Posted::Refused(status),
            0 => Posted::Retry(format!(
                "no answer: {}",
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .next()
                    .unwrap_or("curl failed")
                    .chars()
                    .take(200)
                    .collect::<String>()
            )),
            other => Posted::Retry(format!("HTTP {other}")),
        }
    }
}

/// One sink's retry state: its current pause, and when it is next due.
#[derive(Clone, Copy, Debug, Default)]
pub struct Retry {
    pause: Duration,
    due: Option<std::time::Instant>,
}

enum Posted {
    Delivered,
    Refused(u16),
    Retry(String),
}

/// Run `sink`'s delivery until `stop` is set, waking at least every second for records another
/// process queued.
pub fn spawn_worker(
    sink: Arc<EventSink>,
    stop: Arc<AtomicBool>,
    log: impl Fn(String) + Send + 'static,
) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("chat-event-sinks".to_owned())
        .spawn(move || {
            let mut retry = vec![Retry::default(); sink.config.sinks.len()];
            while !stop.load(Ordering::SeqCst) {
                let wait = sink.deliver(&mut retry, &log);
                let deadline = std::time::Instant::now() + wait;
                while !stop.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })
}

trait PrivateMode {
    fn mode_private(&mut self) -> &mut Self;
}

impl PrivateMode for fs::OpenOptions {
    fn mode_private(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt as _;
        self.mode(0o600)
    }
}

fn private_dirs(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

/// The queue's records, oldest first: names this module writes, never a temporary.
fn queued(queue: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found: Vec<PathBuf> = match fs::read_dir(queue) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.ends_with(".json")
                            && name.as_bytes().first().is_some_and(u8::is_ascii_digit)
                    })
            })
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    found.sort();
    Ok(found)
}

/// How long ago the record at `path` was queued, by the time its name begins with.
fn record_age(path: &Path) -> Option<Duration> {
    let millis: u64 = path
        .file_name()?
        .to_str()?
        .split('-')
        .next()?
        .parse()
        .ok()?;
    Some(Duration::from_millis(
        crate::chat_runtime::unix_millis().saturating_sub(millis),
    ))
}

fn read_config(path: &Path) -> Result<Option<SinksFile>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    // It chooses an executable to run: nobody but its owner may have written it.
    if metadata.permissions().mode() & 0o022 != 0 {
        return Err("others than its owner may write it (chmod 600)".to_owned());
    }
    if std::os::unix::fs::MetadataExt::uid(&metadata) != unsafe { libc::geteuid() } {
        return Err("it is owned by another user".to_owned());
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(format!("it is larger than {MAX_CONFIG_BYTES} bytes"));
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let config: SinksFile = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    config.validate()?;
    Ok(Some(config))
}

fn read_token(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err("it is readable by others than its owner (chmod 600)".to_owned());
    }
    if metadata.len() > MAX_TOKEN_BYTES {
        return Err("it is too large".to_owned());
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let token = text.trim();
    if token.is_empty()
        || token
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\')
    {
        return Err("it does not hold one token on one line".to_owned());
    }
    Ok(token.to_owned())
}

/// `text` inside a curl configuration's double quotes.
fn curl_quote(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `millis` as an RFC 3339 instant in UTC, to the millisecond.
fn rfc3339_millis(millis: u64) -> String {
    format!(
        "{}T{}.{:03}Z",
        crate::inbox::utc_date(millis),
        crate::inbox::utc_clock(millis).trim_end_matches('Z'),
        millis % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentctl-event-sink-{tag}-{}-{}",
            std::process::id(),
            crate::chat_runtime::unix_millis()
        ));
        private_dirs(&dir).expect("a directory");
        dir
    }

    fn write_private(path: &Path, text: &str) {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode_private()
            .open(path)
            .expect("a file");
        file.write_all(text.as_bytes()).expect("written");
    }

    /// A fake `curl` that appends its standard input (its configuration) and the record it
    /// names to `log`, and answers `status`.
    fn fake_curl(dir: &Path, status: u16) -> PathBuf {
        let path = dir.join("fake-curl");
        let log = dir.join("curl.log");
        let script = format!(
            "#!/bin/sh\nconfig=$(cat)\nrecord=$(printf '%s\\n' \"$config\" | sed -n 's/^data-binary = \"@\\(.*\\)\"$/\\1/p')\nprintf '%s\\n' \"$config\" >> '{log}'\ncat \"$record\" >> '{log}'\nprintf '\\n' >> '{log}'\nprintf '{status}'\n",
            log = log.display()
        );
        fs::write(&path, script).expect("the fake");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("executable");
        path
    }

    fn configure(dir: &Path, curl: &Path, extra: &str) {
        write_private(&dir.join("sink.token"), "secret-token-1\n");
        write_private(
            &dir.join(SINKS_FILE),
            &format!(
                r#"{{"version": 1, "sender": {{"id": "U0BRIDGE01", "name": "bridge"}},
                    "sinks": [{{"name": "reader", "url": "https://reader.example/api/v1/live/tail",
                    "token_file": "sink.token", "kinds": ["message.posted", "reaction.added"],
                    "curl": {{"executable": "{}"}}{extra}}}]}}"#,
                curl.display()
            ),
        );
    }

    #[test]
    fn a_post_and_a_reaction_are_queued_and_delivered_in_order_with_the_token_off_the_command_line()
    {
        let dir = temp_dir("deliver");
        let curl = fake_curl(&dir, 202);
        configure(&dir, &curl, "");
        let sink = EventSink::load(&dir, "coordinator").expect("configured");
        sink.record_posted(
            "C0CHANNEL1",
            "C0CHANNEL1/messages/1793377517.953000",
            Some("C0CHANNEL1/threads/1793377304.077519"),
            "a reply",
            "req-1",
        );
        sink.record_reaction(
            "C0CHANNEL1",
            "C0CHANNEL1/messages/1793377304.077519",
            "✅",
            "req-2",
        );
        let queue = dir.join(SINKS_DIRECTORY).join("reader").join("queue");
        assert_eq!(queued(&queue).expect("listed").len(), 2);
        let mut backoff = vec![Retry::default()];
        sink.deliver(&mut backoff, &|_| {});
        assert!(queued(&queue).expect("listed").is_empty(), "both delivered");
        let log = fs::read_to_string(dir.join("curl.log")).expect("curl ran");
        assert!(log.contains("header = \"Authorization: Bearer secret-token-1\""));
        let records: Vec<Value> = log
            .lines()
            .filter(|line| line.starts_with('{'))
            .map(|line| serde_json::from_str(line).expect("a record"))
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["schema"], SCHEMA);
        assert_eq!(records[0]["kind"], "message.posted");
        assert_eq!(records[0]["event_id"], "message.posted:req-1");
        assert_eq!(
            records[0]["thread_id"],
            "C0CHANNEL1/threads/1793377304.077519"
        );
        assert_eq!(records[0]["text"], "a reply");
        assert_eq!(records[0]["sender"]["id"], "U0BRIDGE01");
        assert_eq!(records[0]["bridge"], "coordinator");
        assert_eq!(records[1]["kind"], "reaction.added");
        assert_eq!(records[1]["emoji"], "✅");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sink_that_cannot_take_records_keeps_them_and_drops_the_oldest_past_its_bound_counting_them(
    ) {
        let dir = temp_dir("bound");
        let curl = fake_curl(&dir, 503);
        configure(&dir, &curl, r#", "max_queued": 2"#);
        let sink = EventSink::load(&dir, "coordinator").expect("configured");
        for n in 0..3 {
            sink.record_posted(
                "C0CHANNEL1",
                &format!("C0CHANNEL1/messages/1.00000{n}"),
                None,
                "x",
                &format!("r{n}"),
            );
        }
        let queue = dir.join(SINKS_DIRECTORY).join("reader").join("queue");
        assert_eq!(queued(&queue).expect("listed").len(), 2, "bounded");
        let mut backoff = vec![Retry::default()];
        let wait = sink.deliver(&mut backoff, &|_| {});
        assert!(wait <= Duration::from_secs(1));
        assert!(backoff[0].due.is_some(), "due again after a pause");
        assert_eq!(queued(&queue).expect("listed").len(), 2, "kept for a retry");
        let dropped: Value = serde_json::from_slice(
            &fs::read(
                dir.join(SINKS_DIRECTORY)
                    .join("reader")
                    .join("dropped.json"),
            )
            .expect("counted"),
        )
        .expect("json");
        assert_eq!(dropped["dropped"], 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_the_sink_refuses_for_good_is_dropped_and_the_next_is_still_sent() {
        let dir = temp_dir("refused");
        let curl = fake_curl(&dir, 400);
        configure(&dir, &curl, "");
        let sink = EventSink::load(&dir, "coordinator").expect("configured");
        sink.record_posted(
            "C0CHANNEL1",
            "C0CHANNEL1/messages/1.000001",
            None,
            "x",
            "r1",
        );
        sink.record_posted(
            "C0CHANNEL1",
            "C0CHANNEL1/messages/1.000002",
            None,
            "y",
            "r2",
        );
        let mut backoff = vec![Retry::default()];
        sink.deliver(&mut backoff, &|_| {});
        let queue = dir.join(SINKS_DIRECTORY).join("reader").join("queue");
        assert!(queued(&queue).expect("listed").is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_token_others_can_read_or_a_malformed_file_turns_nothing_on_and_breaks_nothing() {
        let dir = temp_dir("guards");
        let curl = fake_curl(&dir, 202);
        configure(&dir, &curl, "");
        fs::set_permissions(dir.join("sink.token"), fs::Permissions::from_mode(0o644))
            .expect("chmod");
        let sink = EventSink::load(&dir, "coordinator").expect("configured");
        sink.record_posted(
            "C0CHANNEL1",
            "C0CHANNEL1/messages/1.000001",
            None,
            "x",
            "r1",
        );
        let mut backoff = vec![Retry::default()];
        sink.deliver(&mut backoff, &|_| {});
        let queue = dir.join(SINKS_DIRECTORY).join("reader").join("queue");
        assert_eq!(
            queued(&queue).expect("listed").len(),
            1,
            "never sent with that token"
        );
        assert!(!dir.join("curl.log").exists());
        write_private(
            &dir.join(SINKS_FILE),
            r#"{"version": 1, "sinks": [{"name": "Bad Name"}]}"#,
        );
        assert!(EventSink::load(&dir, "coordinator").is_none());
        write_private(
            &dir.join(SINKS_FILE),
            r#"{"version": 1, "sinks": [{"name": "x", "url": "https://h/", "token_file": "../t", "kinds": ["message.posted"]}]}"#,
        );
        assert!(
            EventSink::load(&dir, "coordinator").is_none(),
            "a token outside the state directory"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sink_that_cannot_be_reached_holds_back_no_other() {
        let dir = temp_dir("two-sinks");
        let down = fake_curl(&dir, 503);
        let up_dir = dir.join("up");
        private_dirs(&up_dir).expect("a directory");
        let up = fake_curl(&up_dir, 202);
        write_private(&dir.join("sink.token"), "secret-token-1\n");
        write_private(
            &dir.join(SINKS_FILE),
            &format!(
                r#"{{"version": 1, "sinks": [
                    {{"name": "down", "url": "https://down.example/", "token_file": "sink.token",
                      "kinds": ["message.posted"], "curl": {{"executable": "{}"}}}},
                    {{"name": "up", "url": "https://up.example/", "token_file": "sink.token",
                      "kinds": ["message.posted"], "curl": {{"executable": "{}"}}}}]}}"#,
                down.display(),
                up.display()
            ),
        );
        let sink = EventSink::load(&dir, "coordinator").expect("configured");
        let mut retry = vec![Retry::default(); 2];
        sink.record_posted(
            "C0CHANNEL1",
            "C0CHANNEL1/messages/1.000001",
            None,
            "x",
            "r1",
        );
        sink.deliver(&mut retry, &|_| {});
        sink.record_posted(
            "C0CHANNEL1",
            "C0CHANNEL1/messages/1.000002",
            None,
            "y",
            "r2",
        );
        // The first sink is not due yet; the second takes the new record at once.
        sink.deliver(&mut retry, &|_| {});
        let queue = |name: &str| dir.join(SINKS_DIRECTORY).join(name).join("queue");
        assert_eq!(queued(&queue("down")).expect("listed").len(), 2);
        assert!(queued(&queue("up")).expect("listed").is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_configuration_others_may_write_turns_nothing_on() {
        let dir = temp_dir("config-mode");
        let curl = fake_curl(&dir, 202);
        configure(&dir, &curl, "");
        assert!(EventSink::load(&dir, "coordinator").is_some());
        fs::set_permissions(dir.join(SINKS_FILE), fs::Permissions::from_mode(0o664))
            .expect("chmod");
        assert!(EventSink::load(&dir, "coordinator").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn queue_names_order_by_time_then_writer_then_count() {
        let counter = AtomicU64::new(9);
        let ninth = unique_suffix(&counter);
        let tenth = unique_suffix(&counter);
        assert!(ninth < tenth, "{ninth} sorts before {tenth}");
    }

    #[test]
    fn instants_are_rfc3339_utc_to_the_millisecond() {
        assert_eq!(
            rfc3339_millis(1_793_377_517_953),
            "2026-10-30T16:25:17.953Z"
        );
    }
}
