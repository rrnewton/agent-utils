//! ACTIVE/ARCHIVED shard-pair validation parity.
//!
//! Composes the shard readers with the partial-shard pairing rule the
//! existing state bootstrap applies: a lone non-empty shard refuses as
//! partial machine state, a lone empty shard pairs with an empty
//! counterpart, and two present shards are both validated. Snapshot mode
//! only: when the append-only event log derives the state, derivation
//! stays with `rebuild`. The log is classified by its entries (a
//! complete event file derives, any other entry refuses with the
//! event loader's message, and an in-progress write of the first
//! event file refuses with the bootstrap's partial-update message,
//! since the bootstrap would append that file). Read-only;
//! the missing counterpart is reported as the empty document the
//! bootstrap would write, never created.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::{load_cfg, parse_state};
use crate::archive_state::parse_archive;
use crate::global_state::{
    event_log_status, partial_write_refusal, validate_event_chain, EventLogStatus,
};

type Result<T> = std::result::Result<T, String>;

fn present(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// Validate the ACTIVE and ARCHIVED shards for one machine as a pair,
/// printing both normalized documents.
pub(crate) fn parse_states(root: Option<&Path>, machine_override: Option<&str>) -> Result<Value> {
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let events: PathBuf = cfg.control.join(format!("EVENTS.{selected}"));
    match event_log_status(&events) {
        EventLogStatus::Derived => {
            validate_event_chain(&events, &selected)?;
            let active = crate::hold_write::active_document(root, &cfg, &selected)?;
            let archive = crate::recover_absent_write::archives_document(&cfg, &selected)?;
            return Ok(json!({
                "machine": selected,
                "active": active,
                "archive": archive,
            }));
        }
        EventLogStatus::Invalid(message) => return Err(message),
        EventLogStatus::Snapshot => {}
    }
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    let archive_path = cfg.control.join(format!("ARCHIVED.{selected}.json"));
    let empty_active = json!({"schema": 2, "machine": selected, "revision": 0, "slots": []});
    let empty_archive = json!({"schema": 2, "machine": selected, "revision": 0, "records": []});
    let (active, archive) = match (present(&active_path), present(&archive_path)) {
        (false, false) => (empty_active, empty_archive),
        (true, false) => {
            let active = parse_state(root, machine_override)?;
            let non_empty = active.get("revision").and_then(Value::as_i64) != Some(0)
                || active
                    .get("slots")
                    .and_then(Value::as_array)
                    .is_some_and(|slots| !slots.is_empty());
            if non_empty {
                return Err(format!(
                    "partial machine state for {selected}: non-empty ACTIVE has no ARCHIVED"
                ));
            }
            (active, empty_archive)
        }
        (false, true) => {
            let archive = parse_archive(root, machine_override)?;
            let non_empty = archive.get("revision").and_then(Value::as_i64) != Some(0)
                || archive
                    .get("records")
                    .and_then(Value::as_array)
                    .is_some_and(|records| !records.is_empty());
            if non_empty {
                return Err(format!(
                    "partial machine state for {selected}: non-empty ARCHIVED has no ACTIVE"
                ));
            }
            (empty_active, archive)
        }
        (true, true) => (
            parse_state(root, machine_override)?,
            parse_archive(root, machine_override)?,
        ),
    };
    if let Some(message) = partial_write_refusal(&events) {
        return Err(message);
    }
    Ok(json!({
        "machine": selected,
        "active": active,
        "archive": archive,
    }))
}
