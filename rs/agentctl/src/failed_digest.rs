//! Once-per-entry digest of a queue's `failed/` entries (#903 queue-failed-digest).
//!
//! A delivery cancelled before it was typed, or one whose submission could
//! not be proved, lands in `failed/` as `possibly_submitted` and is never
//! retried. Before this module nothing told the recipient: the message was
//! silently parked. The digest tells the agent, once per entry, that such an
//! entry exists — it never re-delivers the message itself, which could
//! duplicate a submission that did in fact arrive.
//!
//! Composer-evidence false failures (`left the composer without visible
//! submission evidence`, #896) are included but labelled `may have arrived`:
//! for those, the prompt demonstrably left the composer, so the agent very
//! likely saw it. Every other failed entry is labelled
//! `may not have reached you`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// File (in the queue root, beside the phase directories) recording which
/// failed entries a digest has already named. Keeping it outside `failed/`
/// keeps the digest bookkeeping itself out of the failed listing.
pub const SHOWN_FILE: &str = "failed-digest-shown.json";
/// Queue-item flag marking an item that is itself a digest, so a digest that
/// fails is never digested in turn.
pub const DIGEST_FLAG: &str = "failed_digest";
/// How many entries one digest names before it counts the rest.
pub const MAX_NAMED: usize = 5;
/// Longest first line, in characters, that one digest line carries.
pub const FIRST_LINE_CHARS: usize = 80;
/// Hard bound on one digest's length, in characters.
pub const DIGEST_CHARS: usize = 1_000;

/// One `failed/` entry as a digest or listing names it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FailedEntry {
    /// Message identifier (file name without `.json`).
    pub id: String,
    /// Who sent it: a queue sender field, else the sender header stamped on
    /// the text, else `unknown`.
    pub sender: String,
    /// First meaningful line of the message, bounded to [`FIRST_LINE_CHARS`].
    pub first_line: String,
    /// `may have arrived` for composer-evidence failures, else
    /// `may not have reached you`.
    pub label: String,
    /// The recorded failure detail, when a sidecar or the item carries one.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub error: String,
    /// When the entry failed (Unix seconds), newest first in listings.
    #[serde(default)]
    pub failed_at: i64,
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Whether `error` is a composer-evidence false failure (#896): the prompt
/// left the composer, but no visible submission evidence was found.
#[must_use]
pub fn may_have_arrived(error: &str) -> bool {
    error.contains("without visible submission evidence") || error.contains("left the composer")
}

fn sender_of(document: &serde_json::Map<String, Value>, text: &str) -> String {
    for key in ["sender", "from"] {
        if let Some(sender) = document.get(key).and_then(Value::as_str) {
            if !sender.trim().is_empty() {
                return sender.to_owned();
            }
        }
    }
    if let Some(first) = text.lines().next() {
        if let Some(header) = crate::sender_header::parse(first.trim()) {
            return header.sender;
        }
    }
    "unknown".to_owned()
}

fn first_line_of(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // The sender header names the sender separately; it is not content.
        if crate::sender_header::parse(line).is_some() {
            continue;
        }
        return truncate_chars(line, FIRST_LINE_CHARS);
    }
    String::new()
}

fn read_failed_at(sidecar: &Value, document: &serde_json::Map<String, Value>, path: &Path) -> i64 {
    if let Some(at) = sidecar.get("failed_at").and_then(Value::as_f64) {
        return at as i64;
    }
    if let Some(at) = document.get("delivery_failed_at").and_then(Value::as_f64) {
        return at as i64;
    }
    if let Some(at) = document.get("queued_at").and_then(Value::as_f64) {
        return at as i64;
    }
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Every digestible entry in `root/failed`, newest first. Digest items
/// themselves, `.error` sidecars and unreadable non-items are skipped; an
/// item whose JSON cannot be read still appears, named from its raw text.
#[must_use]
pub fn failed_entries(root: &Path) -> Vec<FailedEntry> {
    let failed = root.join(agent_queue::queue::FAILED);
    let Ok(read) = fs::read_dir(&failed) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = read
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".error.json"))
        })
        .collect();
    paths.sort();
    let mut entries = Vec::new();
    for path in paths {
        let Some(id) = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
        else {
            continue;
        };
        let sidecar = fs::read_to_string(format!("{}.error", path.display()))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or(Value::Null);
        let raw = fs::read_to_string(&path).unwrap_or_default();
        let document = serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|value| value.as_object().cloned());
        if document
            .as_ref()
            .is_some_and(|doc| doc.get(DIGEST_FLAG) == Some(&Value::Bool(true)))
        {
            continue;
        }
        let (sender, first_line, failed_at) = match &document {
            Some(doc) => {
                let text = doc.get("text").and_then(Value::as_str).unwrap_or("");
                (
                    sender_of(doc, text),
                    first_line_of(text),
                    read_failed_at(&sidecar, doc, &path),
                )
            }
            None => (
                "unknown".to_owned(),
                first_line_of(&raw),
                read_failed_at(&sidecar, &serde_json::Map::new(), &path),
            ),
        };
        let error = sidecar
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                document.as_ref().and_then(|doc| {
                    doc.get("delivery_error")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
            })
            .unwrap_or_default();
        entries.push(FailedEntry {
            id,
            sender,
            first_line,
            label: if may_have_arrived(&error) {
                "may have arrived".to_owned()
            } else {
                "may not have reached you".to_owned()
            },
            error,
            failed_at,
        });
    }
    entries.sort_by(|a, b| b.failed_at.cmp(&a.failed_at).then_with(|| b.id.cmp(&a.id)));
    entries
}

fn shown_path(root: &Path) -> PathBuf {
    root.join(SHOWN_FILE)
}

/// Identifiers a digest has already named, from [`SHOWN_FILE`].
#[must_use]
pub fn shown_ids(root: &Path) -> BTreeMap<String, f64> {
    let Ok(text) = fs::read_to_string(shown_path(root)) else {
        return BTreeMap::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return BTreeMap::new();
    };
    value
        .get("shown")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(id, at)| (id.clone(), at.as_f64().unwrap_or(0.0)))
                .collect()
        })
        .unwrap_or_default()
}

/// Failed entries no digest has named yet, newest first.
#[must_use]
pub fn undigested_entries(root: &Path) -> Vec<FailedEntry> {
    let shown = shown_ids(root);
    failed_entries(root)
        .into_iter()
        .filter(|entry| !shown.contains_key(&entry.id))
        .collect()
}

/// Record that a digest named `ids`, so no later digest names them again.
///
/// # Errors
///
/// The queue root cannot be written.
pub fn record_shown(root: &Path, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut shown = shown_ids(root);
    let now = agent_queue::queue::unix_seconds();
    for id in ids {
        shown.entry(id.clone()).or_insert(now);
    }
    let value = json!({"schema": "agentctl-failed-digest/v1", "shown": shown});
    crate::agent::atomic_json(&shown_path(root), &value).map_err(|error| error.to_string())
}

/// One digest line for `entry`: `<label>: <id> <sender> <first line>`.
#[must_use]
pub fn digest_line(entry: &FailedEntry) -> String {
    let mut line = format!("{}: {} {}", entry.label, entry.id, entry.sender);
    if !entry.first_line.is_empty() {
        line.push(' ');
        line.push_str(&entry.first_line);
    }
    line
}

/// The digest text for `entries` (already newest first): at most
/// [`MAX_NAMED`] named entries, a count of the rest, and at most
/// [`DIGEST_CHARS`] characters in total. `None` when there is nothing to say.
#[must_use]
pub fn digest_text(entries: &[FailedEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = entries.iter().take(MAX_NAMED).map(digest_line).collect();
    if entries.len() > MAX_NAMED {
        lines.push(format!(
            "+{} more failed {} in this queue; see `agentctl status`",
            entries.len() - MAX_NAMED,
            if entries.len() - MAX_NAMED == 1 {
                "entry"
            } else {
                "entries"
            }
        ));
    }
    let mut text = String::new();
    for line in lines {
        let candidate = if text.is_empty() {
            line.clone()
        } else {
            format!("{text}\n{line}")
        };
        if candidate.chars().count() > DIGEST_CHARS {
            break;
        }
        text = candidate;
    }
    if text.is_empty() {
        text = truncate_chars(&digest_line(&entries[0]), DIGEST_CHARS);
    }
    Some(text)
}

/// The digest an idle agent should see next, without recording it: the
/// caller records the named entries with [`record_shown`] once the digest
/// is durably queued for that agent.
#[must_use]
pub fn pending_digest(root: &Path) -> Option<(String, Vec<String>)> {
    let entries = undigested_entries(root);
    let text = digest_text(&entries)?;
    let named: Vec<String> = entries
        .iter()
        .take(MAX_NAMED)
        .map(|entry| entry.id.clone())
        .collect();
    Some((text, named))
}

/// Queue one digest prompt for the agent that owns `root`, and record the
/// entries it names as shown. The digest item carries [`DIGEST_FLAG`] so a
/// failed digest is never itself digested. Returns the queued identifier,
/// or `None` when there is nothing undigested.
///
/// # Errors
///
/// The queue cannot be written.
pub fn queue_digest(root: &Path) -> Result<Option<String>, String> {
    let Some((text, named)) = pending_digest(root) else {
        return Ok(None);
    };
    let identifier =
        agent_queue::queue::enqueue(root, None, false, &agent_queue::fsio::Blocking, |id| {
            json!({
                "id": id,
                "text": text,
                DIGEST_FLAG: true,
                "failed_ids": named,
                "queued_at": agent_queue::queue::unix_seconds(),
                "delivery_attempts": 0,
            })
        })
        .map_err(|error| error.to_string())?;
    record_shown(root, &named)?;
    Ok(Some(identifier))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write_failed(root: &Path, id: &str, text: &str, error: &str, failed_at: i64) {
        let failed = root.join("failed");
        fs::create_dir_all(&failed).unwrap();
        let mut file = fs::File::create(failed.join(format!("{id}.json"))).unwrap();
        write!(
            file,
            "{}",
            json!({"id": id, "text": text, "queued_at": failed_at - 10})
        )
        .unwrap();
        fs::write(
            failed.join(format!("{id}.json.error")),
            json!({"artifact": format!("{id}.json"), "outcome": "possibly_submitted", "error": error, "failed_at": failed_at}).to_string(),
        )
        .unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("failed-digest-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_cancelled_before_typing_entry_is_named_once_per_entry() {
        let root = temp_root("once");
        // Simulates #884: cancelled before typing, parked in failed/ as
        // possibly_submitted with nothing telling the recipient.
        write_failed(
            &root,
            "msg-1",
            "[from coordb via agentctl · reply: agentctl send coordb]\nland the parser fix today",
            "control command was cancelled",
            200,
        );
        write_failed(
            &root,
            "msg-2",
            "plain prompt with no header\nsecond line",
            "prompt left the composer without visible submission evidence",
            300,
        );
        let entries = failed_entries(&root);
        assert_eq!(entries.len(), 2);
        // Newest first.
        assert_eq!(entries[0].id, "msg-2");
        assert_eq!(entries[0].label, "may have arrived");
        assert_eq!(entries[1].id, "msg-1");
        assert_eq!(entries[1].label, "may not have reached you");
        assert_eq!(entries[1].sender, "coordb");
        assert_eq!(entries[1].first_line, "land the parser fix today");

        let (text, named) = pending_digest(&root).expect("digest");
        assert!(
            text.contains("may not have reached you: msg-1 coordb land the parser fix today"),
            "{text}"
        );
        assert!(text.contains("may have arrived: msg-2"), "{text}");
        // The original message is never re-delivered: only its first line.
        assert!(!text.contains("second line"), "{text}");

        record_shown(&root, &named).unwrap();
        assert!(pending_digest(&root).is_none(), "digest is once per entry");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_digest_is_bounded_and_counts_the_rest() {
        let root = temp_root("bounded");
        for index in 0..9 {
            write_failed(
                &root,
                &format!("msg-{index}"),
                &format!("line {index} {}", "x".repeat(200)),
                "cancelled",
                100 + index as i64,
            );
        }
        let (text, _named) = pending_digest(&root).expect("digest");
        assert!(text.chars().count() <= DIGEST_CHARS, "{}", text.len());
        assert!(text.contains("+4 more failed entries"), "{text}");
        for line in text.lines() {
            assert!(line.chars().count() <= 200, "{line}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn queue_digest_queues_once_and_never_digests_itself() {
        let root = temp_root("queue");
        agent_queue::queue::prepare(&root).unwrap();
        write_failed(&root, "msg-1", "hello world", "cancelled", 100);
        let first = queue_digest(&root).unwrap().expect("queued");
        assert!(root.join("inbox").join(format!("{first}.json")).is_file());
        assert!(queue_digest(&root).unwrap().is_none(), "once per entry");
        // Even if the digest item itself lands in failed/, it is excluded.
        let item = fs::read_to_string(root.join("inbox").join(format!("{first}.json"))).unwrap();
        fs::create_dir_all(root.join("failed")).unwrap();
        fs::write(root.join("failed").join(format!("{first}.json")), item).unwrap();
        assert_eq!(failed_entries(&root).len(), 1, "only the real entry");
        let _ = fs::remove_dir_all(&root);
    }
}
