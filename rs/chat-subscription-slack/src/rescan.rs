//! The periodic rescan's durable ledger and the text it adds to a message with files.
//!
//! The event stream does not carry every message: a message with an attached file never arrives
//! as `message.created` (#415 slack-file-share-delivery; #485 chat-file-messages found nine of
//! the owner's screenshots dropped over four days). So besides the tail, the plugin periodically
//! lists the channel's recent history and active threads and admits any allowed-sender message
//! the stream did not carry, through the same admission path (see [`crate::plugin`]).
//!
//! The bridge accepts a message key only once, and refuses a known key whose content differs, so
//! the rescan must never offer a message the bridge already has: one the stream carried (whose
//! text a listing renders independently) or one the rescan already delivered (which may have been
//! edited since). And after an admission the bridge rolled back it requires the identical batch
//! again. This module keeps `slack-rescan-ledger.json` beside the plugin's diagnostics log:
//!
//! * `tracking_since_micros`: when the ledger began. The rescan never rescues a message created
//!   before it, since an older message may have been carried before anything was recorded.
//! * `carried`: messages the stream carried in a configured channel, `C…/<ts>`. The rescan skips
//!   them; the stream itself still replays them, since a stream batch is rebuilt from its event.
//! * `rescued`: messages the rescan delivered and the bridge acknowledged. The rescan skips them,
//!   and a stream event that carries one late becomes a checkpoint.
//! * `pending`: the one rescued batch offered and not yet acknowledged, saved before it is
//!   offered. A subscription that resumes from the cursor before it offers it again, identical,
//!   before anything else; one that resumes from its cursor records it rescued.
//! * `cursor`: the last cursor the bridge acknowledged. A subscription that resumes from any
//!   other (an older plugin, or one without a ledger, moved the bridge on in between, so messages
//!   it delivered were not recorded) restarts `tracking_since_micros` at its start.
//!
//! The ledger is kept, and carried messages recorded, even while the rescan itself is off, so
//! turning it on later never offers a message the stream already delivered.
//!
//! Entries are pruned by message age: carried ones past the rescan window, rescued ones past a day
//! (longer than the stream's retention of about eight hours, so a replayed stream event meets the
//! same entry). Each list holds at most [`MAX_LEDGER_ENTRIES`]. The ledger holds identifiers and,
//! for the pending batch, its message, which an acknowledgement removes.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chat_subscription::{
    ChannelId, InboundMessage, MessageId, ProviderPayload, SenderId, ThreadId,
};
use serde_json::{json, Map, Value};

use crate::ids;

/// The ledger's file name, beside the diagnostics log.
pub const LEDGER_FILE: &str = "slack-rescan-ledger.json";
/// The ledger's schema identifier.
pub const LEDGER_SCHEMA: &str = "agentctl.slack.rescan.v1";
/// Default `rescan_interval_seconds`.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(90);
/// Default `rescan_window_seconds`: a day, so a reply in a thread started yesterday is found.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
/// A message younger than this is left to the stream, which normally carries it within a second.
pub const GRACE_MILLIS: u64 = 15_000;
/// History pages (20 messages each) one pass reads per channel, newest first.
pub const MAX_HISTORY_PAGES: usize = 5;
/// Threads one pass reads per channel; the rest wait for the next pass.
pub const MAX_THREADS_PER_PASS: usize = 8;
/// Pages one pass reads per thread; a longer thread continues on the next pass.
pub const MAX_THREAD_PAGES: usize = 2;
/// Messages one pass queues for delivery; the rest are found again by the next pass.
pub const MAX_RESCUES_PER_PASS: usize = 32;
/// One pass stops starting listings after this long.
pub const PASS_BUDGET: Duration = Duration::from_secs(60);
/// The next pass after one that yielded to a live event starts this soon.
pub const YIELD_RETRY: Duration = Duration::from_secs(15);
/// The longest wait after consecutive failed passes.
pub const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);
/// In-pass retries of one rate-limited listing.
pub const MAX_RATE_LIMIT_RETRIES: usize = 3;
/// Longest single in-pass wait for a rate-limited listing; a longer `retry_after` is left to
/// the next pass's scheduling instead.
pub const MAX_RATE_LIMIT_RETRY_WAIT: Duration = Duration::from_secs(15);
/// Longest wait before the next pass after a rate-limited pass.
pub const MAX_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(5 * 60);

/// The wait Slack asked for before retrying a rate-limited listing, when `error` is one and
/// names it (`retry_after_seconds` in the client's error document, or "Retry after N seconds"
/// in its message). `None` for any other failure, including a rate limit that names no wait.
#[must_use]
pub fn rate_limit_delay(error: &crate::transport::TransportError) -> Option<Duration> {
    let code = error.code.to_ascii_lowercase();
    let detail = error.detail.to_ascii_lowercase();
    let limited = (code.contains("rate") && code.contains("limit"))
        || detail.contains("rate-limited")
        || detail.contains("rate_limited")
        || detail.contains("ratelimited");
    if !limited {
        return None;
    }
    let seconds = [
        "retry_after_seconds",
        "retry_after",
        "retry-after",
        "retry after",
    ]
    .iter()
    .find_map(|marker| parse_seconds_after(&detail, marker))?;
    Some(Duration::from_secs(seconds.min(MAX_BACKOFF.as_secs())))
}

fn parse_seconds_after(text: &str, marker: &str) -> Option<u64> {
    // The number must follow the marker directly (after JSON punctuation), so digits
    // elsewhere in the detail can never invent a wait.
    let rest = text.split(marker).nth(1)?;
    let digits: String = rest
        .trim_start_matches(['"', ':', ' ', '='])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}
/// Most entries in each ledger list.
pub const MAX_LEDGER_ENTRIES: usize = 4_096;
/// Where the download commands in a file note save files.
pub const DOWNLOAD_DIRECTORY: &str = "/tmp/agentctl-slack-files/";
const RESCUED_RETENTION_MICROS: u128 = 25 * 60 * 60 * 1_000_000;
const CARRIED_MARGIN_MICROS: u128 = 60 * 60 * 1_000_000;
const RESCUE_SUFFIX: &str = ";rescued:";

/// The ledger key of one message.
#[must_use]
pub fn message_key(channel_id: &str, ts: &str) -> String {
    format!("{channel_id}/{ts}")
}

fn key_micros(key: &str) -> Option<u128> {
    ids::ts_micros(key.rsplit_once('/')?.1)
}

/// The cursor of a rescued batch: the cursor before it, without any earlier rescue suffix, then
/// the message. A subscription resumed from it resumes the stream where the base cursor does.
#[must_use]
pub fn rescue_cursor(base: &str, key: &str) -> String {
    format!("{}{RESCUE_SUFFIX}{key}", strip_rescue(base))
}

/// The stream cursor a possibly rescued cursor resumes from.
#[must_use]
pub fn strip_rescue(cursor: &str) -> &str {
    cursor
        .split_once(RESCUE_SUFFIX)
        .map_or(cursor, |(base, _)| base)
}

/// One rescued batch offered and not yet acknowledged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRescue {
    /// The cursor the bridge held when the batch was offered.
    pub prior_cursor: Option<String>,
    /// The batch's cursor.
    pub cursor: String,
    /// The message's ledger key.
    pub key: String,
    /// The delivered message, exactly.
    pub message: InboundMessage,
}

fn inbound_to_json(message: &InboundMessage) -> Value {
    json!({
        "channel_id": message.channel_id().as_str(),
        "message_id": message.message_id().as_str(),
        "thread_id": message.thread_id().as_str(),
        "sender_id": message.sender_id().as_str(),
        "text": message.text(),
        "created_at": message.created_at(),
        "thread_reply": message.is_thread_reply(),
        "payload": message.provider_payload().map(|payload| json!({
            "schema": payload.schema(),
            "data": payload.data(),
        })),
    })
}

fn inbound_from_json(value: &Value) -> Option<InboundMessage> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let message = InboundMessage::new(
        ChannelId::new(text("channel_id")?).ok()?,
        MessageId::new(text("message_id")?).ok()?,
        ThreadId::new(text("thread_id")?).ok()?,
        SenderId::new(text("sender_id")?).ok()?,
        text("text")?,
        text("created_at")?,
        value.get("thread_reply")?.as_bool()?,
    )
    .ok()?;
    match value.get("payload") {
        None | Some(Value::Null) => Some(message),
        Some(payload) => Some(
            message.with_provider_payload(
                ProviderPayload::new(
                    payload.get("schema")?.as_str()?,
                    payload.get("data")?.as_object()?.clone(),
                )
                .ok()?,
            ),
        ),
    }
}

impl PendingRescue {
    fn to_json(&self) -> Value {
        json!({
            "prior_cursor": self.prior_cursor,
            "cursor": self.cursor,
            "key": self.key,
            "message": inbound_to_json(&self.message),
        })
    }

    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            prior_cursor: match value.get("prior_cursor")? {
                Value::Null => None,
                Value::String(cursor) => Some(cursor.clone()),
                _ => return None,
            },
            cursor: value.get("cursor")?.as_str()?.to_owned(),
            key: value.get("key")?.as_str()?.to_owned(),
            message: inbound_from_json(value.get("message")?)?,
        })
    }
}

/// The rescan's ledger; see the module documentation.
#[derive(Clone, Debug)]
pub struct Ledger {
    path: PathBuf,
    /// When the ledger began, in Unix microseconds.
    pub tracking_since_micros: u128,
    carried: BTreeMap<String, u128>,
    rescued: BTreeMap<String, u128>,
    /// The batch offered and not yet acknowledged.
    pub pending: Option<PendingRescue>,
    /// The last cursor the bridge acknowledged.
    pub cursor: Option<String>,
}

impl Ledger {
    /// The ledger beside `diagnostics_log`, if one is configured.
    #[must_use]
    pub fn path_beside(diagnostics_log: Option<&Path>) -> Option<PathBuf> {
        diagnostics_log
            .and_then(Path::parent)
            .map(|directory| directory.join(LEDGER_FILE))
    }

    /// Load the ledger at `path`. A missing ledger starts now; so does an unreadable one, and the
    /// returned note says why, for the diagnostics log.
    #[must_use]
    pub fn load(path: PathBuf, now_micros: u128) -> (Self, Option<String>) {
        let fresh = |path: PathBuf| Self {
            path,
            tracking_since_micros: now_micros,
            carried: BTreeMap::new(),
            rescued: BTreeMap::new(),
            pending: None,
            cursor: None,
        };
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (fresh(path), None)
            }
            Err(error) => {
                let note =
                    format!("the rescan ledger could not be read ({error}); starting a new one");
                return (fresh(path), Some(note));
            }
        };
        let parsed = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .filter(|value| value.get("schema").and_then(Value::as_str) == Some(LEDGER_SCHEMA))
            .and_then(|value| {
                let keys = |name: &str| -> Option<BTreeMap<String, u128>> {
                    value
                        .get(name)?
                        .as_array()?
                        .iter()
                        .map(|key| {
                            let key = key.as_str()?;
                            Some((key.to_owned(), key_micros(key)?))
                        })
                        .collect()
                };
                Some(Self {
                    path: path.clone(),
                    tracking_since_micros: u128::from(
                        value.get("tracking_since_micros")?.as_u64()?,
                    ),
                    carried: keys("carried")?,
                    rescued: keys("rescued")?,
                    pending: match value.get("pending")? {
                        Value::Null => None,
                        pending => Some(PendingRescue::from_json(pending)?),
                    },
                    cursor: match value.get("cursor") {
                        None | Some(Value::Null) => None,
                        Some(cursor) => Some(cursor.as_str()?.to_owned()),
                    },
                })
            });
        match parsed {
            Some(ledger) => (ledger, None),
            None => (
                fresh(path),
                Some("the rescan ledger was not a valid ledger; starting a new one".to_owned()),
            ),
        }
    }

    /// The ledger's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the stream carried this message.
    #[must_use]
    pub fn carried(&self, key: &str) -> bool {
        self.carried.contains_key(key)
    }

    /// Whether the rescan delivered this message.
    #[must_use]
    pub fn rescued(&self, key: &str) -> bool {
        self.rescued.contains_key(key)
    }

    /// Record a message the stream carried; returns whether that is new.
    pub fn mark_carried(&mut self, key: &str) -> bool {
        match key_micros(key) {
            Some(micros) => self.carried.insert(key.to_owned(), micros).is_none(),
            None => false,
        }
    }

    /// Record a message the rescan delivered; returns whether that is new.
    pub fn mark_rescued(&mut self, key: &str) -> bool {
        match key_micros(key) {
            Some(micros) => self.rescued.insert(key.to_owned(), micros).is_none(),
            None => false,
        }
    }

    /// Forget entries the rescan and the stream's retention can no longer reach.
    pub fn prune(&mut self, now_micros: u128, window: Duration) {
        let carried_floor = now_micros.saturating_sub(window.as_micros() + CARRIED_MARGIN_MICROS);
        let rescued_floor = now_micros.saturating_sub(
            RESCUED_RETENTION_MICROS.max(window.as_micros() + CARRIED_MARGIN_MICROS),
        );
        for (entries, floor) in [
            (&mut self.carried, carried_floor),
            (&mut self.rescued, rescued_floor),
        ] {
            entries.retain(|_, micros| *micros >= floor);
            while entries.len() > MAX_LEDGER_ENTRIES {
                let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, micros)| **micros)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                entries.remove(&oldest);
            }
        }
    }

    /// Replace the ledger file atomically and durably.
    ///
    /// # Errors
    ///
    /// Returns the I/O error; the caller logs it.
    pub fn save(&self) -> std::io::Result<()> {
        let record = json!({
            "schema": LEDGER_SCHEMA,
            "tracking_since_micros": u64::try_from(self.tracking_since_micros).unwrap_or(u64::MAX),
            "carried": self.carried.keys().collect::<Vec<_>>(),
            "rescued": self.rescued.keys().collect::<Vec<_>>(),
            "pending": self.pending.as_ref().map(PendingRescue::to_json),
            "cursor": self.cursor,
        });
        let bytes = serde_json::to_vec_pretty(&record).map_err(std::io::Error::other)?;
        let mut temporary = self.path.clone().into_os_string();
        temporary.push(".tmp");
        let temporary = PathBuf::from(temporary);
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(&temporary, &self.path)?;
        // The pending record must survive a power loss, so the rename itself is made durable.
        match self.path.parent() {
            Some(directory) => fs::File::open(directory)?.sync_all(),
            None => Ok(()),
        }
    }
}

/// Quote one word for a POSIX shell, leaving plain words alone.
#[must_use]
pub fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_./:=@%+,-".contains(&byte));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// The lines appended to a delivered message that has files or reached the bridge through the
/// rescan. `download` is the shell-quoted client command that the stream's tail runs, which acts
/// as the channel member and can read the files; `None` when there is no command-line client.
/// The note is bounded: at most [`crate::cli::MAX_FILE_IDS`] files, and a command of at most a few
/// hundred bytes (the configured path is at most 256), so it stays far below the text limit.
#[must_use]
pub fn delivery_note(
    download: Option<&str>,
    channel_id: &str,
    file_ids: &[String],
    rescued: bool,
) -> String {
    let mut note = String::new();
    if rescued {
        note.push_str(
            "\n\n[agentctl] This message reached the bridge through the periodic rescan; the live \
             event stream did not carry it.",
        );
    }
    if file_ids.is_empty() {
        return note;
    }
    let count = file_ids.len();
    note.push_str(&format!(
        "\n\n[agentctl] This message has {count} attached file{}: {}.",
        if count == 1 { "" } else { "s" },
        file_ids.join(", ")
    ));
    match download {
        Some(command) => {
            note.push_str(&format!(
                " To view {}, download to a temporary path with:",
                if count == 1 { "it" } else { "one" }
            ));
            for file_id in file_ids {
                note.push_str(&format!(
                    "\n{command} slack.file download --file-id={file_id} --channel-id={channel_id} \
                     --output-path={DOWNLOAD_DIRECTORY}"
                ));
            }
            note.push_str(&format!(
                "\nEach saves {DOWNLOAD_DIRECTORY}<file id>-<file name> and prints that path."
            ));
        }
        None => note.push_str(" Download them with a Slack client that can read this channel."),
    }
    note
}

/// The payload fields a delivered message gains: its file IDs and, when rescued, how it came.
pub fn annotate_payload(payload: &mut Map<String, Value>, file_ids: &[String], rescued: bool) {
    if !file_ids.is_empty() {
        payload.insert("file_ids".to_owned(), json!(file_ids));
    }
    if rescued {
        payload.insert("delivered_by".to_owned(), json!("rescan"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescue_cursors_keep_their_stream_base() {
        let cursor = rescue_cursor("slack1:seq:7", "C0C91RW5FAR/1792000801.000100");
        assert_eq!(cursor, "slack1:seq:7;rescued:C0C91RW5FAR/1792000801.000100");
        assert_eq!(strip_rescue(&cursor), "slack1:seq:7");
        assert_eq!(
            rescue_cursor(&cursor, "C0C91RW5FAR/1792000802.000100"),
            "slack1:seq:7;rescued:C0C91RW5FAR/1792000802.000100"
        );
    }

    #[test]
    fn rate_limit_waits_come_from_slacks_own_retry_after() {
        let limited = crate::transport::TransportError::not_applied(
            "slack_rate_limited",
            "Slack rate-limited request conversations.history. Retry after 2 seconds. \
             {\"retry_after_seconds\":2}",
            true,
        );
        assert_eq!(rate_limit_delay(&limited), Some(Duration::from_secs(2)));
        let generic =
            crate::transport::TransportError::not_applied("ratelimited", "slow down", true);
        assert_eq!(rate_limit_delay(&generic), None);
        let other = crate::transport::TransportError::not_applied(
            "client_failed",
            "client exited with exit status: 1",
            true,
        );
        assert_eq!(rate_limit_delay(&other), None);
    }

    #[test]
    fn shell_words_are_quoted_only_when_needed() {
        assert_eq!(
            shell_word("/opt/slack/bin/slack-cli"),
            "/opt/slack/bin/slack-cli"
        );
        assert_eq!(shell_word("/opt/my tools/it's"), r"'/opt/my tools/it'\''s'");
    }

    #[test]
    fn notes_name_every_file_with_its_download_command() {
        let note = delivery_note(
            Some("/opt/slack"),
            "C0C91RW5FAR",
            &["F0AAAAAAAA1".to_owned(), "F0AAAAAAAA2".to_owned()],
            true,
        );
        assert!(note.contains("through the periodic rescan"));
        assert!(note.contains("This message has 2 attached files: F0AAAAAAAA1, F0AAAAAAAA2."));
        assert!(note.contains(
            "\n/opt/slack slack.file download --file-id=F0AAAAAAAA2 --channel-id=C0C91RW5FAR \
             --output-path=/tmp/agentctl-slack-files/"
        ));
        assert_eq!(delivery_note(None, "C0C91RW5FAR", &[], false), "");
    }

    #[test]
    fn the_ledger_round_trips_and_prunes_by_message_age() {
        let directory = std::env::temp_dir().join(format!("slack-rescan-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("directory");
        let path = directory.join(LEDGER_FILE);
        let _ = fs::remove_file(&path);
        let now = 1_792_000_800_000_000_u128;
        let (mut ledger, note) = Ledger::load(path.clone(), now);
        assert!(note.is_none());
        assert!(ledger.mark_carried("C0C91RW5FAR/1792000700.000100"));
        assert!(!ledger.mark_carried("C0C91RW5FAR/1792000700.000100"));
        assert!(ledger.mark_rescued("C0C91RW5FAR/1792000750.000100"));
        // Two days old: past both retentions.
        assert!(ledger.mark_carried("C0C91RW5FAR/1791827900.000100"));
        assert!(ledger.mark_rescued("C0C91RW5FAR/1791827900.000100"));
        ledger.prune(now, Duration::from_secs(3_600));
        ledger.save().expect("save");
        let (loaded, note) = Ledger::load(path.clone(), now + 1);
        assert!(note.is_none());
        assert_eq!(loaded.tracking_since_micros, now);
        assert!(loaded.carried("C0C91RW5FAR/1792000700.000100"));
        assert!(loaded.rescued("C0C91RW5FAR/1792000750.000100"));
        assert!(!loaded.carried("C0C91RW5FAR/1791827900.000100"));
        assert!(!loaded.rescued("C0C91RW5FAR/1791827900.000100"));
        fs::write(&path, b"{not json").expect("corrupt");
        let (fresh, note) = Ledger::load(path, now + 2);
        assert!(note.is_some());
        assert_eq!(fresh.tracking_since_micros, now + 2);
        fs::remove_dir_all(&directory).expect("cleanup");
    }
}
