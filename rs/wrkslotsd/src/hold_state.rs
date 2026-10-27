//! Slot-hold file validation parity.
//!
//! Reproduces the existing slot-hold snapshot loader: `HOLD.<machine>.<slot>.json`
//! under the control directory, absent meaning no hold (null), otherwise
//! exact keys, schema, filename/identity agreement, timestamp, and a
//! non-empty reason, printing the record as stored. Read-only.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::active_state::{as_int, as_mapping, as_str, exact_keys, load_cfg, parse_timestamp};
use crate::config_validate::{refuse_symlink, validate_name};

type Result<T> = std::result::Result<T, String>;

const HOLD_SCHEMA: i64 = 1;

/// Read and validate one slot's hold record, printing it as stored
/// (or null when the slot is not held).
pub(crate) fn parse_hold(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
) -> Result<Value> {
    validate_name(slot, "slot")?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    let path: PathBuf = cfg.control.join(format!(
        "HOLD.{}.{}.{}.json",
        selected.chars().count(),
        selected,
        slot
    ));
    if !path.exists() && !path.is_symlink() {
        return Ok(Value::Null);
    }
    refuse_symlink(&path, "slot hold")?;
    let text = std::fs::read_to_string(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("missing slot hold: {}", path.display())
        } else {
            format!("cannot read slot hold {}: {error}", path.display())
        }
    })?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("slot hold is malformed JSON: {error}"))?;
    let raw = as_mapping(&value, "slot hold")?;
    exact_keys(
        raw,
        &["schema", "machine", "slot", "held_at", "reason"],
        &[],
        "slot hold",
    )?;
    if as_int(&raw["schema"], "slot hold.schema", 0)? != HOLD_SCHEMA {
        return Err(format!(
            "unsupported slot hold schema in {}",
            path.display()
        ));
    }
    if as_str(&raw["machine"], "slot hold.machine")? != selected {
        return Err(format!(
            "slot hold machine does not match its filename: {}",
            path.display()
        ));
    }
    if as_str(&raw["slot"], "slot hold.slot")? != slot {
        return Err(format!(
            "slot hold name does not match its filename: {}",
            path.display()
        ));
    }
    let held_at = as_str(&raw["held_at"], "slot hold.held_at")?;
    parse_timestamp(held_at, "slot hold.held_at")?;
    if as_str(&raw["reason"], "slot hold.reason")?
        .trim()
        .is_empty()
    {
        return Err(format!("slot hold has an empty reason: {}", path.display()));
    }
    Ok(Value::Object(raw.clone()))
}
