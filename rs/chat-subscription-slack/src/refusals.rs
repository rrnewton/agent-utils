//! A durable count of messages from allowed senders that the plugin did not admit.
//!
//! A message an allowed sender wrote is meant to become a request. When the plugin refuses one
//! (another channel, a non-message subtype, a timestamp before the lineage start, an ignored
//! prefix, anything else), the bridge only sees a bare checkpoint, so nothing downstream can tell
//! that a person's message was dropped. On 2026-10-19 three of the owner's thread replies were
//! dropped as bot-authored while every health signal stayed green.
//!
//! This module keeps `allowed-sender-refusals.json` beside the plugin's diagnostics log: the total
//! count, the count by reason, and the most recent refusals with their time. agentctl reads it
//! for `chat status` and raises its delivery alarm while a refusal is recent. The record holds
//! identifiers only, never message text.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

/// The record's file name, beside the diagnostics log.
pub const REFUSALS_FILE: &str = "allowed-sender-refusals.json";
/// The record's schema identifier; agentctl refuses any other.
pub const REFUSALS_SCHEMA: &str = "agentctl.allowed-sender-refusals.v1";
/// How many recent refusals the record keeps.
pub const MAX_RECENT: usize = 32;

/// One refused message from an allowed sender.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refusal {
    /// When the plugin refused it, in Unix milliseconds.
    pub at_millis: u64,
    /// The provider event sequence.
    pub sequence: u64,
    /// The Slack channel the message was in.
    pub channel_id: String,
    /// The message's Slack timestamp.
    pub message_ts: String,
    /// The allowed sender's user ID.
    pub user_id: String,
    /// Why it was not admitted.
    pub reason: String,
}

impl Refusal {
    fn to_json(&self) -> Value {
        json!({
            "at_millis": self.at_millis,
            "sequence": self.sequence,
            "channel_id": self.channel_id,
            "message_ts": self.message_ts,
            "user_id": self.user_id,
            "reason": self.reason,
        })
    }
}

/// Where refusals are saved; without a diagnostics log nothing is saved.
#[derive(Clone, Debug, Default)]
pub struct RefusalRecorder {
    path: Option<PathBuf>,
}

impl RefusalRecorder {
    /// A recorder saving beside `diagnostics_log`, if one is configured.
    #[must_use]
    pub fn beside(diagnostics_log: Option<&Path>) -> Self {
        Self {
            path: diagnostics_log
                .and_then(Path::parent)
                .map(|directory| directory.join(REFUSALS_FILE)),
        }
    }

    /// The record's path, if any.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Add `refusal` to the record: `{"schema", "count", "by_reason", "recent"}`, with `recent`
    /// oldest first and at most [`MAX_RECENT`] long. A record that cannot be read starts again
    /// from zero rather than blocking the event stream; a failed write is returned to the caller,
    /// which only logs it.
    pub fn record(&self, refusal: &Refusal) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let previous = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(|record| record.get("schema").and_then(Value::as_str) == Some(REFUSALS_SCHEMA));
        let count = previous
            .as_ref()
            .and_then(|record| record.get("count"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(1);
        let mut by_reason = previous
            .as_ref()
            .and_then(|record| record.get("by_reason"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_else(Map::new);
        let reason_count = by_reason
            .get(&refusal.reason)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(1);
        by_reason.insert(refusal.reason.clone(), Value::from(reason_count));
        let mut recent = previous
            .as_ref()
            .and_then(|record| record.get("recent"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        recent.push(refusal.to_json());
        if recent.len() > MAX_RECENT {
            let excess = recent.len() - MAX_RECENT;
            recent.drain(..excess);
        }
        let record = json!({
            "schema": REFUSALS_SCHEMA,
            "count": count,
            "by_reason": by_reason,
            "recent": recent,
        });
        let bytes = serde_json::to_vec_pretty(&record).map_err(std::io::Error::other)?;
        let mut temporary = path.clone().into_os_string();
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
        fs::rename(&temporary, path)
    }
}

/// The current time in Unix milliseconds.
#[must_use]
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(sequence: u64, reason: &str) -> Refusal {
        Refusal {
            at_millis: 1,
            sequence,
            channel_id: "C1".to_owned(),
            message_ts: format!("1.{sequence}"),
            user_id: "U1".to_owned(),
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn refusals_accumulate_by_reason_and_keep_the_most_recent() {
        let directory = std::env::temp_dir().join(format!("slack-refusals-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("directory");
        let log = directory.join("plugin.log");
        let recorder = RefusalRecorder::beside(Some(&log));
        let total = MAX_RECENT as u64 + 3;
        for sequence in 0..total {
            let reason = if sequence % 2 == 0 {
                "other channel"
            } else {
                "ignored prefix"
            };
            recorder.record(&refusal(sequence, reason)).expect("record");
        }
        let record: Value =
            serde_json::from_slice(&fs::read(directory.join(REFUSALS_FILE)).expect("read"))
                .expect("parse");
        fs::remove_dir_all(&directory).expect("cleanup");
        assert_eq!(record["schema"], REFUSALS_SCHEMA);
        assert_eq!(record["count"], total);
        assert_eq!(record["by_reason"]["other channel"], 18);
        assert_eq!(record["by_reason"]["ignored prefix"], 17);
        let recent = record["recent"].as_array().expect("recent");
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0]["sequence"], 3);
        assert_eq!(recent[MAX_RECENT - 1]["sequence"], total - 1);
    }

    #[test]
    fn without_a_diagnostics_log_nothing_is_saved() {
        let recorder = RefusalRecorder::beside(None);
        assert!(recorder.path().is_none());
        recorder
            .record(&refusal(1, "other channel"))
            .expect("no-op");
    }
}
