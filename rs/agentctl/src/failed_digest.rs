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
/// Default age window: only entries newer than this are itemised.
/// Older entries are the pre-existing backlog, summarised once.
pub const WINDOW_HOURS: f64 = 24.0;
/// Default minimum minutes between two digests for one queue.
pub const MIN_INTERVAL_MINUTES: f64 = 30.0;
/// Identifier prefix of a digest item itself, so it is recognised (and
/// never digested or listed as a failed message) even if a drain rewrote
/// its file without the [`DIGEST_FLAG`] field.
pub const DIGEST_ID_PREFIX: &str = "failed-digest-";
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

pub(crate) fn sender_of(document: &serde_json::Map<String, Value>, text: &str) -> String {
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

/// Whether `line` is envelope furniture rather than body: the sender
/// header (`[from ... via ...]`), the agent-teams envelope line
/// (`[team message: kind ...; expires ...]`) or its reply footer
/// (`[reply: ...]`). The digest's first line is the first BODY line.
fn is_header_line(line: &str) -> bool {
    crate::sender_header::parse(line).is_some()
        || line.starts_with("[from ")
        || line.starts_with("[team message:")
        || line.starts_with("[reply:")
}

pub(crate) fn first_line_of(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || is_header_line(line) {
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
        if id.starts_with(DIGEST_ID_PREFIX)
            || document
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

/// The whole digest state file: shown ids, when the feature was first
/// enabled for this queue, when the last digest was queued, and whether
/// the pre-existing backlog has been summarised.
fn read_state(root: &Path) -> Value {
    fs::read_to_string(shown_path(root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({"schema": "agentctl-failed-digest/v1", "shown": {}}))
}

fn write_state(root: &Path, state: &Value) -> Result<(), String> {
    crate::agent::atomic_json(&shown_path(root), state).map_err(|error| error.to_string())
}

/// Identifiers a digest has already named, from [`SHOWN_FILE`].
#[must_use]
pub fn shown_ids(root: &Path) -> BTreeMap<String, f64> {
    let value = read_state(root);
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

/// When the digest was first enabled for this queue, recorded the first
/// time any digest function runs: entries older than this are the
/// pre-existing backlog and are summarised once, never itemised.
pub fn enabled_at(root: &Path) -> f64 {
    let state = read_state(root);
    if let Some(at) = state.get("enabled_at").and_then(Value::as_f64) {
        if at > 0.0 {
            return at;
        }
    }
    // Second granularity: an entry that failed in the same second the
    // feature was enabled counts as new, not as pre-existing backlog.
    let now = agent_queue::queue::unix_seconds().floor();
    let mut state = state;
    state["enabled_at"] = json!(now);
    let _ = write_state(root, &state);
    now
}

fn env_hours(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| *value > 0.0)
        .unwrap_or(default)
}

/// The itemising window in seconds: `AGENTCTL_FAILED_DIGEST_WINDOW_HOURS`,
/// else [`WINDOW_HOURS`].
#[must_use]
pub fn window_seconds() -> f64 {
    env_hours("AGENTCTL_FAILED_DIGEST_WINDOW_HOURS", WINDOW_HOURS) * 3600.0
}

/// The minimum seconds between digests: `AGENTCTL_FAILED_DIGEST_MIN_MINUTES`,
/// else [`MIN_INTERVAL_MINUTES`].
#[must_use]
pub fn min_interval_seconds() -> f64 {
    env_hours("AGENTCTL_FAILED_DIGEST_MIN_MINUTES", MIN_INTERVAL_MINUTES) * 60.0
}

fn cutoff(root: &Path, now: f64) -> f64 {
    enabled_at(root).max(now - window_seconds())
}

/// Failed entries no digest has named yet and new enough to itemise
/// (newer than the cutoff), newest first.
#[must_use]
pub fn undigested_entries(root: &Path) -> Vec<FailedEntry> {
    undigested_entries_at(root, agent_queue::queue::unix_seconds())
}

/// [`undigested_entries`] at an explicit `now` (tests).
#[must_use]
pub fn undigested_entries_at(root: &Path, now: f64) -> Vec<FailedEntry> {
    let shown = shown_ids(root);
    let cutoff = cutoff(root, now);
    failed_entries(root)
        .into_iter()
        .filter(|entry| !shown.contains_key(&entry.id))
        .filter(|entry| (entry.failed_at as f64) >= cutoff)
        .collect()
}

/// Undigested entries older than the cutoff: the pre-existing backlog.
fn backlog_entries(root: &Path, now: f64) -> Vec<FailedEntry> {
    let shown = shown_ids(root);
    let cutoff = cutoff(root, now);
    failed_entries(root)
        .into_iter()
        .filter(|entry| !shown.contains_key(&entry.id))
        .filter(|entry| (entry.failed_at as f64) < cutoff)
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
    let mut state = read_state(root);
    let now = agent_queue::queue::unix_seconds();
    let shown = state
        .as_object_mut()
        .expect("digest state is an object")
        .entry("shown")
        .or_insert_with(|| json!({}));
    if let Some(map) = shown.as_object_mut() {
        for id in ids {
            map.entry(id.clone()).or_insert_with(|| json!(now));
        }
    }
    write_state(root, &state)
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
/// newest in-window entries itemised (top [`MAX_NAMED`] plus a count), and
/// the pre-existing backlog summarised in one line — never itemised, and
/// only until it has been summarised once. The caller records the named
/// entries with [`record_shown`] once the digest is durably queued.
#[must_use]
pub fn pending_digest(root: &Path) -> Option<(String, Vec<String>)> {
    pending_digest_at(root, agent_queue::queue::unix_seconds())
}

/// [`pending_digest`] at an explicit `now` (tests).
#[must_use]
pub fn pending_digest_at(root: &Path, now: f64) -> Option<(String, Vec<String>)> {
    let entries = undigested_entries_at(root, now);
    let backlog = backlog_entries(root, now);
    let summarised = read_state(root)
        .get("backlog_summarised")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut named: Vec<String> = entries
        .iter()
        .take(MAX_NAMED)
        .map(|entry| entry.id.clone())
        .collect();
    let mut text = digest_text(&entries).unwrap_or_default();
    if !backlog.is_empty() && !summarised {
        named.extend(backlog.iter().map(|entry| entry.id.clone()));
        let summary = format!(
            "{} older failed {} in this queue {} the digest and are summarised once, not itemised; clear them with `agentctl failed ack NAME --all`",
            backlog.len(),
            if backlog.len() == 1 { "entry" } else { "entries" },
            if backlog.len() == 1 { "predates" } else { "predate" }
        );
        text = if text.is_empty() {
            summary
        } else {
            format!("{text}\n{summary}")
        };
    }
    if named.is_empty() && text.is_empty() {
        return None;
    }
    if !backlog.is_empty() && !summarised && text.is_empty() {
        return None;
    }
    Some((truncate_chars(&text, DIGEST_CHARS), named))
}

/// Queue one digest prompt for the agent that owns `root`, and record the
/// entries it names as shown. At most one digest per
/// [`min_interval_seconds`] per queue (and one per idle window: a second
/// call inside the interval queues nothing). The digest item's identifier
/// carries [`DIGEST_ID_PREFIX`] and the item carries [`DIGEST_FLAG`], so a
/// digest that itself lands in `failed/` is never digested or listed.
/// Returns the queued identifier, or `None` when there is nothing to say
/// yet or the last digest was too recent.
///
/// # Errors
///
/// The queue cannot be written.
pub fn queue_digest(root: &Path) -> Result<Option<String>, String> {
    queue_digest_at(root, agent_queue::queue::unix_seconds())
}

/// [`queue_digest`] at an explicit `now` (tests).
pub fn queue_digest_at(root: &Path, now: f64) -> Result<Option<String>, String> {
    // Claim before typing, atomically, shared across drainers: the whole
    // check-and-record runs under the queue's digest lock, so two
    // drainers (the watcher and an agent's own send path, say) can never
    // both generate a digest naming the same entry. The shown record is
    // written before the digest item is ever typed.
    let lock_path = root.join(".failed-digest.lock");
    let lock = agent_queue::fsio::open_private_lock(&lock_path, "failed digest lock")
        .map_err(|error| error.to_string())?;
    agent_queue::fsio::lock_exclusive(
        &lock,
        &lock_path,
        "failed digest",
        &agent_queue::fsio::Blocking,
    )
    .map_err(|error| error.to_string())?;
    queue_digest_locked(root, now)
}

/// [`queue_digest_at`] with the digest lock held (see there).
fn queue_digest_locked(root: &Path, now: f64) -> Result<Option<String>, String> {
    let _ = enabled_at(root);
    let state = read_state(root);
    if let Some(last) = state.get("last_digest_at").and_then(Value::as_f64) {
        if now - last < min_interval_seconds() {
            return Ok(None);
        }
    }
    let Some((text, named)) = pending_digest_at(root, now) else {
        return Ok(None);
    };
    let digest_id = format!(
        "{DIGEST_ID_PREFIX}{}",
        agent_queue::queue::generated_message_id()
    );
    let summarised = !backlog_entries(root, now).is_empty();
    let identifier = agent_queue::queue::enqueue(
        root,
        Some(&digest_id),
        false,
        &agent_queue::fsio::Blocking,
        |id| {
            json!({
                "id": id,
                // Agentctl's own framing, so a digest typed into a composer
                // is attributable there and flushable with one Enter.
                "text": format!("[agentctl failed-digest]\n{text}"),
                DIGEST_FLAG: true,
                "failed_ids": named,
                "queued_at": now,
                "delivery_attempts": 0,
            })
        },
    )
    .map_err(|error| error.to_string())?;
    record_shown(root, &named)?;
    let mut state = read_state(root);
    state["last_digest_at"] = json!(now);
    if summarised {
        state["backlog_summarised"] = json!(true);
    }
    write_state(root, &state)?;
    Ok(Some(identifier))
}

/// Best-effort delivery, at most once: take an unconfirmed digest item
/// out of circulation after its one attempt. A pending digest is
/// withdrawn — unless it is stranded, meaning its paste was typed and may
/// still show: then it stays pending so a later pass flushes it with one
/// Enter (never retyped) instead of deleting its only record. One a drain
/// filed to `failed/` is set aside in `cancelled/` instead (a digest is
/// never a failed message). An in-flight digest is left for the drain's
/// recovery, which sets digest items aside the same way. Returns whether
/// it acted.
///
/// # Errors
///
/// The queue cannot be read or written.
pub fn discard_unconfirmed(root: &Path, id: &str) -> Result<bool, String> {
    use agent_queue::queue::MessageState;
    match agent_queue::queue::message_state(root, id).map_err(|e| e.to_string())? {
        Some(MessageState::Pending) => {
            // A stranded pending digest was typed: leave it pending so the
            // stranded path can flush it Enter-only. Withdrawing would
            // delete the only record that can attribute its draft.
            for lane in [agent_queue::queue::INBOX, agent_queue::queue::INBOX_V2] {
                let path = root.join(lane).join(format!("{id}.json"));
                let Ok(raw) = fs::read_to_string(&path) else {
                    continue;
                };
                let stranded = serde_json::from_str::<Value>(&raw)
                    .ok()
                    .is_some_and(|doc| doc.get("stranded_at").is_some());
                if stranded {
                    return Ok(false);
                }
                break;
            }
            agent_queue::queue::withdraw(root, id, &agent_queue::fsio::Blocking)
                .map_err(|e| e.to_string())
        }
        Some(MessageState::Failed) => {
            let failed = root.join(agent_queue::queue::FAILED);
            let cancelled = root.join(agent_queue::queue::CANCELLED);
            // Private (0700) like every queue directory, and repairs an
            // existing non-private one: a 0755 queue state directory makes
            // the whole queue refuse every later operation.
            queue_subdir(root, &cancelled)?;
            fs::rename(
                failed.join(format!("{id}.json")),
                cancelled.join(format!("{id}.json")),
            )
            .map_err(|e| format!("cannot set digest {id} aside: {e}"))?;
            let sidecar = failed.join(format!("{id}.json.error"));
            if sidecar.is_file() {
                let _ = fs::rename(&sidecar, cancelled.join(format!("{id}.json.error")));
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The one way digest code creates a directory under a queue: private
/// (0700) like every queue directory, repairing an existing user-owned
/// non-private one. A plain `create_dir_all` would leave 0755 and make
/// the whole queue refuse every later operation.
fn queue_subdir(root: &Path, path: &Path) -> Result<(), String> {
    agent_queue::queue::prepare_lane(root, path).map_err(|error| error.to_string())
}

/// Move failed entries to `failed-archive-<YYYY-MM-DD>/` beside `failed/`
/// (what operators already do by hand), and record them shown so no
/// digest names them again. With `all`, every current failed entry;
/// otherwise exactly `ids`. Returns the identifiers archived.
///
/// # Errors
///
/// An identifier names no failed entry, or a move cannot be done.
pub fn ack_failed(root: &Path, ids: &[String], all: bool) -> Result<Vec<String>, String> {
    let failed = root.join(agent_queue::queue::FAILED);
    let targets: Vec<String> = if all {
        failed_entries(root)
            .into_iter()
            .map(|entry| entry.id)
            .collect()
    } else {
        ids.to_vec()
    };
    if targets.is_empty() {
        return Err("nothing to ack: name at least one failed id or --all".to_owned());
    }
    // Verify every target before creating anything, so a bad name creates
    // no archive directory (and an unregistered agent is refused earlier,
    // in the manager, before its queue is ever touched).
    for id in &targets {
        agent_queue::queue::validate_message_id(id)
            .map_err(|error| format!("invalid failed id {id:?}: {error}"))?;
        if id.starts_with(DIGEST_ID_PREFIX) {
            continue;
        }
        if !failed.join(format!("{id}.json")).is_file() {
            return Err(format!("no failed entry {id} in {}", failed.display()));
        }
    }
    let archive = root.join(format!("failed-archive-{}", archive_date()));
    queue_subdir(root, &archive)?;
    let mut moved = Vec::new();
    for id in &targets {
        agent_queue::queue::validate_message_id(id)
            .map_err(|error| format!("invalid failed id {id:?}: {error}"))?;
        if id.starts_with(DIGEST_ID_PREFIX) {
            continue;
        }
        let source = failed.join(format!("{id}.json"));
        if !source.is_file() {
            return Err(format!("no failed entry {id} in {}", failed.display()));
        }
        fs::rename(&source, archive.join(format!("{id}.json")))
            .map_err(|error| format!("cannot archive {id}: {error}"))?;
        let sidecar = failed.join(format!("{id}.json.error"));
        if sidecar.is_file() {
            let _ = fs::rename(&sidecar, archive.join(format!("{id}.json.error")));
        }
        moved.push(id.clone());
    }
    record_shown(root, &moved)?;
    Ok(moved)
}

/// Today's date as `YYYY-MM-DD` (UTC) for the archive directory name.
fn archive_date() -> String {
    let days = (agent_queue::queue::unix_seconds() / 86_400.0).floor() as i64;
    // Civil-from-days algorithm (Howard Hinnant's, public domain).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02}")
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

    fn enable_since(root: &Path, at: i64) {
        fs::write(
            root.join(SHOWN_FILE),
            json!({"schema": "agentctl-failed-digest/v1", "shown": {}, "enabled_at": at})
                .to_string(),
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

        enable_since(&root, 1);
        let (text, named) = pending_digest_at(&root, 400.0).expect("digest");
        assert!(
            text.contains("may not have reached you: msg-1 coordb land the parser fix today"),
            "{text}"
        );
        assert!(text.contains("may have arrived: msg-2"), "{text}");
        // The original message is never re-delivered: only its first line.
        assert!(!text.contains("second line"), "{text}");

        record_shown(&root, &named).unwrap();
        assert!(
            pending_digest_at(&root, 400.0).is_none(),
            "digest is once per entry"
        );
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
        enable_since(&root, 1);
        let (text, _named) = pending_digest_at(&root, 1_000.0).expect("digest");
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
        enable_since(&root, 1);
        let first = queue_digest_at(&root, 1_000.0).unwrap().expect("queued");
        assert!(first.starts_with(DIGEST_ID_PREFIX), "{first}");
        assert!(root.join("inbox").join(format!("{first}.json")).is_file());
        let queued: Value = serde_json::from_str(
            &fs::read_to_string(root.join("inbox").join(format!("{first}.json"))).unwrap(),
        )
        .unwrap();
        assert!(
            queued["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("[agentctl failed-digest]\n")),
            "{queued}"
        );
        assert!(
            queue_digest_at(&root, 1_000.0).unwrap().is_none(),
            "once per entry"
        );
        // Even if the digest item itself lands in failed/, it is excluded.
        let item = fs::read_to_string(root.join("inbox").join(format!("{first}.json"))).unwrap();
        fs::create_dir_all(root.join("failed")).unwrap();
        fs::write(root.join("failed").join(format!("{first}.json")), item).unwrap();
        assert_eq!(failed_entries(&root).len(), 1, "only the real entry");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pre_existing_backlog_is_summarised_once_never_itemised() {
        let root = temp_root("backlog");
        agent_queue::queue::prepare(&root).unwrap();
        for index in 0..79 {
            write_failed(&root, &format!("old-{index}"), "old news", "cancelled", 100);
        }
        // First run records the baseline now: every entry predates it.
        let now = agent_queue::queue::unix_seconds();
        let (text, named) = pending_digest_at(&root, now).expect("summary digest");
        assert!(text.contains("79 older failed entries"), "{text}");
        assert!(!text.contains("old-0"), "backlog is not itemised: {text}");
        assert_eq!(named.len(), 79, "the summary records them all shown");
        let _ = queue_digest_at(&root, now).unwrap().expect("queued once");
        assert!(
            pending_digest_at(&root, now + 7_200.0).is_none(),
            "summarised once only"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn at_most_one_digest_per_min_interval() {
        let root = temp_root("rate");
        agent_queue::queue::prepare(&root).unwrap();
        enable_since(&root, 1);
        write_failed(&root, "msg-1", "first", "cancelled", 900);
        assert!(queue_digest_at(&root, 1_000.0).unwrap().is_some());
        write_failed(&root, "msg-2", "second", "cancelled", 1_001);
        assert!(
            queue_digest_at(&root, 1_060.0).unwrap().is_none(),
            "no second digest inside the interval"
        );
        assert!(
            queue_digest_at(&root, 1_000.0 + min_interval_seconds() + 1.0)
                .unwrap()
                .is_some(),
            "one more after the interval"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ack_archives_entries_so_they_stop_appearing() {
        let root = temp_root("ack");
        agent_queue::queue::prepare(&root).unwrap();
        write_failed(&root, "msg-1", "one", "cancelled", 100);
        write_failed(&root, "msg-2", "two", "cancelled", 200);
        let moved = ack_failed(&root, &["msg-1".to_owned()], false).unwrap();
        assert_eq!(moved, ["msg-1"]);
        assert_eq!(failed_entries(&root).len(), 1);
        let archives: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("failed-archive-")
            })
            .collect();
        assert_eq!(archives.len(), 1);
        assert!(archives[0].path().join("msg-1.json").is_file());
        assert!(ack_failed(&root, &["msg-1".to_owned()], false).is_err());
        let moved = ack_failed(&root, &[], true).unwrap();
        assert_eq!(moved, ["msg-2"]);
        assert!(failed_entries(&root).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_first_line_is_the_first_body_line() {
        let root = temp_root("bodyline");
        write_failed(
            &root,
            "tm-1",
            "[from sim@host-b via agent-teams \u{b7} to: recv@host-a \u{b7} id: tm-1]\n[team message: kind info; expires 2026-01-01; sender proved on host-b]\nthe actual body line\n[reply: agent-teams reply tm-1 --file FILE]",
            "cancelled",
            100,
        );
        let entries = failed_entries(&root);
        assert_eq!(entries[0].first_line, "the actual body line");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn two_concurrent_drainers_show_each_entry_once() {
        // The watcher and an agent's own send path can both reach the
        // digest at the same idle moment: the claim lock means exactly
        // one digest item is queued and each entry is shown once.
        let root = temp_root("concurrent");
        agent_queue::queue::prepare(&root).unwrap();
        enable_since(&root, 1);
        write_failed(&root, "msg-1", "shown once", "cancelled", 900);
        let root_a = root.clone();
        let root_b = root.clone();
        let (a, b) = std::thread::scope(|scope| {
            let first = scope.spawn(move || queue_digest_at(&root_a, 1_000.0));
            let second = scope.spawn(move || queue_digest_at(&root_b, 1_000.0));
            (first.join().unwrap(), second.join().unwrap())
        });
        let queued: Vec<_> = [a, b].into_iter().filter_map(|r| r.unwrap()).collect();
        assert_eq!(queued.len(), 1, "exactly one drainer queues a digest");
        let inbox_count = fs::read_dir(root.join("inbox")).unwrap().count();
        assert_eq!(inbox_count, 1, "one digest item in the inbox");
        assert_eq!(
            shown_ids(&root).len(),
            1,
            "shown recorded once, before typing"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unconfirmed_digest_is_discarded_never_retried_or_failed() {
        let root = temp_root("discard");
        agent_queue::queue::prepare(&root).unwrap();
        enable_since(&root, 1);
        write_failed(&root, "msg-1", "real one", "cancelled", 900);
        let id = queue_digest_at(&root, 1_000.0).unwrap().expect("digest");
        // Its one attempt did not confirm: it is withdrawn, not retried.
        assert!(discard_unconfirmed(&root, &id).unwrap());
        assert!(
            agent_queue::queue::message_state(&root, &id)
                .unwrap()
                .is_none(),
            "digest is gone"
        );
        // Even if a drain filed it to failed/, it is set aside in
        // cancelled/ instead and never appears as a failed entry.
        write_failed(
            &root,
            &id,
            "may not have reached you: msg-1",
            "late_paste_lost",
            950,
        );
        assert!(discard_unconfirmed(&root, &id).unwrap());
        assert!(failed_entries(&root).iter().all(|e| e.id != id));
        assert!(root.join("cancelled").join(format!("{id}.json")).is_file());
        // The set-aside path leaves the queue reachable: cancelled/ is
        // private (0700) like every queue directory, so the queue's own
        // privacy check still passes.
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(root.join("failed"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700, "cancelled/ must be private");
        }
        assert!(
            agent_queue::queue::validate_existing_queue(&root).is_ok(),
            "queue stays reachable after a set-aside"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_stranded_pending_digest_is_not_withdrawn() {
        // Its paste was typed and may still show: deleting the record
        // would leave the draft unattributable in the composer forever.
        let root = temp_root("stranded-discard");
        agent_queue::queue::prepare(&root).unwrap();
        enable_since(&root, 1);
        write_failed(&root, "msg-1", "real one", "cancelled", 900);
        let id = queue_digest_at(&root, 1_000.0).unwrap().expect("digest");
        let path = root.join("inbox").join(format!("{id}.json"));
        let mut item: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        item["stranded_at"] = json!(1_000.0);
        fs::write(&path, item.to_string()).unwrap();
        assert!(!discard_unconfirmed(&root, &id).unwrap(), "left pending");
        assert_eq!(
            agent_queue::queue::message_state(&root, &id).unwrap(),
            Some(agent_queue::queue::MessageState::Pending)
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn every_directory_under_a_queue_is_private_after_digest_set_aside_and_ack() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("modes");
        agent_queue::queue::prepare(&root).unwrap();
        enable_since(&root, 1);
        write_failed(&root, "msg-1", "one", "cancelled", 900);
        write_failed(&root, "msg-2", "two", "cancelled", 950);
        // Digest + set-aside paths.
        let id = queue_digest_at(&root, 1_000.0).unwrap().expect("digest");
        assert!(discard_unconfirmed(&root, &id).unwrap());
        // Ack path (creates failed-archive-<date>/).
        assert_eq!(
            ack_failed(&root, &["msg-2".to_owned()], false).unwrap(),
            ["msg-2"]
        );
        // Walk the whole queue tree: every directory is 0700.
        fn walk(dir: &Path, bad: &mut Vec<String>) {
            let mode = fs::metadata(dir).unwrap().permissions().mode() & 0o777;
            if mode != 0o700 {
                bad.push(format!("{} {mode:04o}", dir.display()));
            }
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, bad);
                }
            }
        }
        let mut bad = Vec::new();
        walk(&root, &mut bad);
        assert!(bad.is_empty(), "non-private queue dirs: {bad:?}");
        assert!(agent_queue::queue::validate_existing_queue(&root).is_ok());
        // A pre-existing 0755 queue subdir (from the old careless create)
        // is reported by the pure queue check, then repaired by the next
        // writer open (`prepare`), so the queue is reachable again.
        fs::set_permissions(root.join("failed"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(agent_queue::queue::validate_existing_queue(&root).is_err());
        agent_queue::queue::prepare(&root).unwrap();
        assert!(agent_queue::queue::validate_existing_queue(&root).is_ok());
        let mode = fs::metadata(root.join("failed"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "repaired by the writer open");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_digest_item_in_failed_is_never_digested_even_without_its_flag() {
        let root = temp_root("selfdigest");
        agent_queue::queue::prepare(&root).unwrap();
        write_failed(
            &root,
            "failed-digest-123",
            "may not have reached you: msg-9 somebody hello",
            "cancelled",
            100,
        );
        write_failed(&root, "msg-1", "real one", "cancelled", 200);
        let entries = failed_entries(&root);
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].id, "msg-1");
        let _ = fs::remove_dir_all(&root);
    }
}
