//! Retire pending queue entries by hand (`agentctl queue retire`).
//!
//! When a prompt was delivered outside the queue (typed into the pane
//! directly while delivery was broken), its queued copy must never be
//! typed later as a duplicate. Retiring moves a pending item to
//! `cancelled/` with the operator's reason recorded, through the same
//! tombstone primitive as `agentctl cancel` ([`agent_queue::queue::cancel`]),
//! so no drainer of any version types it afterwards. Retiring an entry
//! that is already cancelled is a no-op, so the command is idempotent.

use std::fs;
use std::path::Path;

use serde::Serialize;

/// One pending queue item, as a dry-run listing names it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PendingEntry {
    /// Message identifier (file name without `.json`).
    pub id: String,
    /// When the item was queued (Unix seconds), when its record says.
    pub queued_at: Option<i64>,
}

/// What retiring one entry did, or would do under `--dry-run`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RetireEntry {
    /// Message identifier.
    pub id: String,
    /// `retired`, `already_cancelled`, `tombstoned`, `inflight`,
    /// `too_late`, `unknown`, or `would_retire` (dry run only).
    pub outcome: &'static str,
}

/// Pending entries in `root`'s `inbox/` and `inbox.v2/`, oldest first.
#[must_use]
pub fn pending_entries(root: &Path) -> Vec<PendingEntry> {
    let mut entries = Vec::new();
    for lane in [agent_queue::queue::INBOX, agent_queue::queue::INBOX_V2] {
        let Ok(read) = fs::read_dir(root.join(lane)) else {
            continue;
        };
        for path in read
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        {
            let Some(id) = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
            else {
                continue;
            };
            let queued_at = fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|value| value.get("queued_at").and_then(serde_json::Value::as_i64));
            entries.push(PendingEntry { id, queued_at });
        }
    }
    entries.sort_by(|a, b| {
        a.queued_at
            .unwrap_or(i64::MAX)
            .cmp(&b.queued_at.unwrap_or(i64::MAX))
            .then_with(|| a.id.cmp(&b.id))
    });
    entries
}

/// Whether `outcome` means the entry will never be typed from the queue.
fn settled(outcome: &str) -> bool {
    matches!(outcome, "retired" | "already_cancelled" | "tombstoned")
}

/// Retire the pending entries named by `ids`, or every pending entry
/// when `all_pending`. With `dry_run`, nothing moves and pending
/// entries report `would_retire`. The returned entries are in target
/// order; check [`RetireEntry::outcome`] (or [`all_settled`]) per entry.
///
/// # Errors
///
/// An identifier is malformed, no target was selected, or the queue
/// refused a move.
pub fn retire(
    root: &Path,
    ids: &[String],
    all_pending: bool,
    reason: &str,
    dry_run: bool,
) -> Result<Vec<RetireEntry>, String> {
    let pending = pending_entries(root);
    let targets: Vec<String> = if all_pending {
        pending.iter().map(|entry| entry.id.clone()).collect()
    } else {
        ids.to_vec()
    };
    if targets.is_empty() {
        return Err("nothing to retire: name at least one --entry or --all-pending".to_owned());
    }
    for id in &targets {
        agent_queue::queue::validate_message_id(id)
            .map_err(|error| format!("invalid message id {id:?}: {error}"))?;
    }
    let mut entries = Vec::new();
    for id in &targets {
        let outcome = if dry_run {
            if pending.iter().any(|entry| &entry.id == id) {
                "would_retire"
            } else if root
                .join(agent_queue::queue::CANCELLED)
                .join(format!("{id}.json"))
                .is_file()
            {
                "already_cancelled"
            } else if [agent_queue::queue::PROCESSED, agent_queue::queue::FAILED]
                .iter()
                .any(|phase| root.join(phase).join(format!("{id}.json")).is_file())
            {
                "too_late"
            } else {
                "unknown"
            }
        } else {
            match agent_queue::queue::cancel(root, id, reason)
                .map_err(|error| format!("cannot retire {id}: {error}"))?
            {
                agent_queue::queue::CancelOutcome::SetAside => "retired",
                agent_queue::queue::CancelOutcome::AlreadyCancelled => "already_cancelled",
                agent_queue::queue::CancelOutcome::Pending => "tombstoned",
                agent_queue::queue::CancelOutcome::Inflight => "inflight",
                agent_queue::queue::CancelOutcome::TooLate => "too_late",
                agent_queue::queue::CancelOutcome::Unknown => "unknown",
            }
        };
        entries.push(RetireEntry {
            id: id.clone(),
            outcome,
        });
    }
    Ok(entries)
}

/// Whether every entry in `entries` is settled (never to be typed),
/// counting a dry run's `would_retire` as settled for listing purposes.
#[must_use]
pub fn all_settled(entries: &[RetireEntry]) -> bool {
    entries
        .iter()
        .all(|entry| settled(entry.outcome) || entry.outcome == "would_retire")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    fn temp_queue() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agentctl-queue-retire-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn write_pending(root: &Path, lane: &str, id: &str, queued_at: i64) {
        let dir = root.join(lane);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{id}.json")),
            serde_json::json!({"id": id, "text": "hi", "queued_at": queued_at}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn retire_moves_a_pending_entry_to_cancelled_with_the_reason() {
        let root = temp_queue();
        write_pending(&root, "inbox", "msg-1", 10);
        write_pending(&root, "inbox.v2", "msg-2", 20);
        let entries = retire(
            &root,
            &["msg-1".to_owned()],
            false,
            "delivered by hand",
            false,
        )
        .unwrap();
        assert_eq!(entries[0].outcome, "retired");
        assert!(all_settled(&entries));
        let moved = fs::read_to_string(root.join("cancelled/msg-1.json")).unwrap();
        assert!(moved.contains("delivered by hand"), "{moved}");
        assert!(!root.join("inbox/msg-1.json").exists());
        // The other pending entry is untouched.
        assert!(root.join("inbox.v2/msg-2.json").exists());
        // Idempotent: retiring it again is already_cancelled, still settled.
        let again = retire(&root, &["msg-1".to_owned()], false, "again", false).unwrap();
        assert_eq!(again[0].outcome, "already_cancelled");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dry_run_lists_without_moving_and_all_pending_takes_every_lane() {
        let root = temp_queue();
        write_pending(&root, "inbox", "msg-1", 10);
        write_pending(&root, "inbox.v2", "msg-2", 5);
        let listed = pending_entries(&root);
        assert_eq!(
            listed.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["msg-2", "msg-1"]
        );
        let dry = retire(&root, &[], true, "", true).unwrap();
        assert_eq!(dry.len(), 2);
        assert!(dry.iter().all(|e| e.outcome == "would_retire"));
        assert!(root.join("inbox/msg-1.json").exists());
        let all = retire(&root, &[], true, "delivered by hand", false).unwrap();
        assert!(all.iter().all(|e| e.outcome == "retired"));
        assert!(pending_entries(&root).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unknown_entry_is_reported_not_retired() {
        let root = temp_queue();
        fs::create_dir_all(root.join("inbox")).unwrap();
        let entries = retire(&root, &["nope-1".to_owned()], false, "x", false).unwrap();
        assert_eq!(entries[0].outcome, "unknown");
        assert!(!all_settled(&entries));
        let _ = fs::remove_dir_all(&root);
    }
}
