//! Registry storage inconsistency parity (filesystem scope).
//!
//! Reproduces the filesystem half of the existing storage diagnosis:
//! slot-directory presence and safety, nested checkout entries, and
//! managed-directory enumeration. Git worktree registration/identity
//! checks and slot-image mount findings stay with the existing
//! `status`; a registry with slot-image records refuses here rather
//! than risk misreporting an unmounted image. Read-only.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::{load_cfg, parse_state_ordered, Cfg};
use crate::global_state::discover_machines;

type Result<T> = std::result::Result<T, String>;

#[allow(clippy::too_many_arguments)]
fn note(
    kind: &str,
    scope: &str,
    slot: Option<&str>,
    slot_type: Option<&str>,
    machine: Option<&str>,
    checkout: Option<&str>,
    detail: String,
    remedy: &str,
) -> Value {
    json!({
        "kind": kind,
        "scope": scope,
        "slot": slot,
        "slot_type": slot_type,
        "machine": machine,
        "checkout": checkout,
        "detail": detail,
        "remedy": remedy,
    })
}

/// Format a filesystem name the way Python's `!r` formats a string.
pub(crate) fn py_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut output = String::new();
    output.push(quote);
    for ch in value.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            ch if ch == quote => {
                output.push('\\');
                output.push(ch);
            }
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if (ch as u32) < 0x20 || (ch as u32) == 0x7f => {
                output.push_str(&format!("\\x{:02x}", ch as u32));
            }
            ch => output.push(ch),
        }
    }
    output.push(quote);
    output
}

pub(crate) fn managed_root(cfg: &Cfg) -> PathBuf {
    if cfg
        .worktrees
        .file_name()
        .is_some_and(|name| name == "slots")
    {
        cfg.worktrees
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    } else {
        cfg.worktrees.clone()
    }
}

pub(crate) fn slot_root(cfg: &Cfg, slot_type: &str) -> PathBuf {
    if slot_type == "validate" {
        managed_root(cfg).join("validate")
    } else {
        cfg.worktrees.clone()
    }
}

pub(crate) fn storage_sort_key(value: &Value) -> (String, String, String, String, String) {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    (
        text("machine"),
        text("slot_type"),
        text("slot"),
        text("checkout"),
        text("kind"),
    )
}

/// Diagnose filesystem drift between the active rows and the managed
/// slot directories, printing the sorted findings.
pub(crate) fn parse_storage(root: Option<&Path>) -> Result<Value> {
    let (cfg, _selected) = load_cfg(root, None)?;
    let images = cfg.control.join("slot-images");
    let has_image_records = images.is_dir()
        && std::fs::read_dir(&images)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
    if has_image_records {
        return Err(
            "slot image records are present; storage diagnosis stays with the Python status"
                .to_string(),
        );
    }
    let machines = discover_machines(&cfg.control)?;
    let mut findings: Vec<Value> = Vec::new();
    let mut expected: Vec<(String, BTreeSet<String>)> = vec![
        ("agent".to_string(), BTreeSet::new()),
        ("validate".to_string(), BTreeSet::new()),
    ];
    let mut records: Vec<(String, String, String, Value)> = Vec::new();
    for machine in &machines {
        let (active, order) = parse_state_ordered(root, Some(machine))?;
        let slots = active
            .get("slots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let by_slot: std::collections::HashMap<String, Value> = slots
            .into_iter()
            .filter_map(|slot| {
                let name = slot.get("slot")?.as_str()?.to_string();
                Some((name, slot))
            })
            .collect();
        for item in &order {
            let stored = by_slot.get(&item.slot).cloned().unwrap_or(Value::Null);
            records.push((
                machine.clone(),
                item.slot.clone(),
                item.slot_type.clone(),
                stored,
            ));
        }
    }
    records.sort_by(|left, right| (&left.0, &left.1, &left.2).cmp(&(&right.0, &right.1, &right.2)));
    for (machine, slot, slot_type, stored) in &records {
        for (known_type, slots) in expected.iter_mut() {
            if known_type == slot_type {
                slots.insert(slot.clone());
            }
        }
        let slot_path = slot_root(&cfg, slot_type).join(slot);
        let present = slot_path.symlink_metadata().is_ok();
        let safe = slot_path.is_dir()
            && !slot_path
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
        if !safe {
            let (kind, detail) = if present {
                (
                    "unsafe-slot-directory",
                    format!(
                        "active row points at an unsafe slot path {}",
                        slot_path.display()
                    ),
                )
            } else {
                (
                    "row-without-directory",
                    format!(
                        "active row but no slot directory at {}",
                        slot_path.display()
                    ),
                )
            };
            findings.push(note(
                kind,
                "row",
                Some(slot),
                Some(slot_type),
                Some(machine),
                None,
                detail,
                "preserve the active row; restore its exact slot directory or use the supported recovery command after proving the storage is intentionally absent",
            ));
            continue;
        }
        let layout = stored
            .get("layout")
            .and_then(Value::as_str)
            .unwrap_or(&cfg.layout);
        if layout == "nested" {
            let wanted: BTreeSet<String> = stored
                .get("checkouts")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.get("name").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            match std::fs::read_dir(&slot_path) {
                Err(error) => findings.push(note(
                    "slot-unreadable",
                    "row",
                    Some(slot),
                    Some(slot_type),
                    Some(machine),
                    None,
                    format!("cannot inspect {}: {error}", slot_path.display()),
                    "make the recorded slot directory readable, then rerun 'wrkslots status'",
                )),
                Ok(entries) => {
                    let mut actual: BTreeSet<String> = BTreeSet::new();
                    let mut entry_error: Option<std::io::Error> = None;
                    for entry in entries {
                        match entry {
                            Ok(entry) => {
                                actual.insert(entry.file_name().to_string_lossy().into_owned());
                            }
                            Err(error) => {
                                entry_error = Some(error);
                                break;
                            }
                        }
                    }
                    if let Some(error) = entry_error {
                        findings.push(note(
                            "slot-unreadable",
                            "row",
                            Some(slot),
                            Some(slot_type),
                            Some(machine),
                            None,
                            format!("cannot inspect {}: {error}", slot_path.display()),
                            "make the recorded slot directory readable, then rerun 'wrkslots status'",
                        ));
                        continue;
                    }
                    let mut allowed = wanted.clone();
                    if slot_type == "agent" {
                        allowed.insert("HANDOFF.md".to_string());
                    }
                    for name in wanted.difference(&actual) {
                        findings.push(note(
                            "missing-checkout",
                            "row",
                            Some(slot),
                            Some(slot_type),
                            Some(machine),
                            Some(name),
                            format!(
                                "record names checkout {} but it is not in {}",
                                py_repr(name),
                                slot_path.display()
                            ),
                            "preserve the row and repair or recover the named checkout before mutation",
                        ));
                    }
                    for name in actual.difference(&allowed) {
                        findings.push(note(
                            "unexpected-entry",
                            "row",
                            Some(slot),
                            Some(slot_type),
                            Some(machine),
                            Some(name),
                            format!(
                                "slot holds {}, which its record does not name",
                                py_repr(name)
                            ),
                            "inspect and register or relocate the unexpected entry; do not delete it to make the registry look consistent",
                        ));
                    }
                }
            }
        }
    }
    for (slot_type, wanted) in &expected {
        let root_path = slot_root(&cfg, slot_type);
        if root_path.symlink_metadata().is_err() {
            continue;
        }
        let is_symlink = root_path
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink || !root_path.is_dir() {
            findings.push(note(
                "unsafe-slots-directory",
                "directory",
                None,
                Some(slot_type),
                None,
                None,
                format!(
                    "managed {slot_type} slots directory is unsafe: {}",
                    root_path.display()
                ),
                "restore the configured managed directory as a real directory before mutation",
            ));
            continue;
        }
        let entries = match std::fs::read_dir(&root_path) {
            Ok(entries) => entries,
            Err(error) => {
                findings.push(note(
                    "slots-directory-unreadable",
                    "directory",
                    None,
                    Some(slot_type),
                    None,
                    None,
                    format!("cannot enumerate {}: {error}", root_path.display()),
                    "make the configured managed directory readable, then rerun 'wrkslots status'",
                ));
                continue;
            }
        };
        let mut actual: BTreeSet<String> = BTreeSet::new();
        let mut collected: Vec<std::fs::DirEntry> = Vec::new();
        let mut entry_error: Option<std::io::Error> = None;
        for entry in entries {
            match entry {
                Ok(entry) => collected.push(entry),
                Err(error) => {
                    entry_error = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = entry_error {
            findings.push(note(
                "slots-directory-unreadable",
                "directory",
                None,
                Some(slot_type),
                None,
                None,
                format!("cannot enumerate {}: {error}", root_path.display()),
                "make the configured managed directory readable, then rerun 'wrkslots status'",
            ));
            continue;
        }
        for entry in collected {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_entry = path.is_dir()
                || path
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false);
            if !is_entry || name.starts_with("EVENTS.") {
                continue;
            }
            if path == cfg.control.join("wrkslots") {
                continue;
            }
            if root_path == cfg.control
                && ["slot-images", "slot-state", "box-state", "wrkslots-salvage"]
                    .contains(&name.as_str())
            {
                continue;
            }
            if slot_type == "agent" && path == slot_root(&cfg, "validate") {
                continue;
            }
            actual.insert(name);
        }
        for slot in actual.difference(wanted) {
            let remedy = if slot_type == "agent" {
                format!(
                    "inspect the directory, then run 'wrkslots register {slot} --help' to record a live agent slot"
                )
            } else {
                "inspect the retained validation directory, then run 'wrkslots import-existing --help'".to_string()
            };
            findings.push(note(
                "directory-without-row",
                "directory",
                Some(slot),
                Some(slot_type),
                None,
                None,
                format!(
                    "{} exists but no active {slot_type} row claims it",
                    root_path.join(slot).display()
                ),
                &remedy,
            ));
        }
    }
    findings.sort_by_key(storage_sort_key);
    Ok(json!({
        "machines": machines,
        "findings": findings,
    }))
}
