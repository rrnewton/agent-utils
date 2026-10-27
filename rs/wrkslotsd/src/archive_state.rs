//! `ARCHIVED.<machine>.json` validation parity: read one archived-slot shard.
//!
//! Reproduces the existing archive loader for a fixture root: strict JSON
//! reading with symlink refusal, exact keys, per-record retirement evidence
//! checks, historical-import consistency, and checkout path/authority
//! against the validated configuration, emitting records as stored.
//! Read-only; no repository or storage existence is inspected.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::{
    as_int, as_list, as_mapping, as_str, checkout_from_obj, exact_keys,
    historical_task_and_purpose, import_from_obj, landed_ref_for_remote, load_cfg, parse_timestamp,
    posix_relative, slot_directory, Cfg,
};
use crate::config_validate::{refuse_symlink, validate_layout, validate_name};

type Result<T> = std::result::Result<T, String>;

const SCHEMA: i64 = 2;

fn archive_from_obj(cfg: &Cfg, value: &Value, selected: &str, label_prefix: &str) -> Result<Value> {
    let raw = as_mapping(value, label_prefix)?;
    exact_keys(
        raw,
        &["schema", "machine", "revision", "records"],
        &[],
        "archive state",
    )?;
    if as_int(&raw["schema"], "archive state.schema", 0)? != SCHEMA {
        return Err(format!(
            "unsupported archive state schema in {label_prefix}"
        ));
    }
    let actual_machine = as_str(&raw["machine"], "archive state.machine")?;
    if actual_machine != selected {
        return Err(format!(
            "archive state machine mismatch: filename says {selected}, content says {actual_machine}"
        ));
    }
    let mut records: Vec<Value> = Vec::new();
    let mut archive_ids: Vec<String> = Vec::new();
    let mut archived_slots: Vec<String> = Vec::new();
    for (index, item) in as_list(&raw["records"], "archive state.records")?
        .iter()
        .enumerate()
    {
        let label = format!("archive state.records[{index}]");
        let mapping = as_mapping(item, &label)?;
        exact_keys(
            mapping,
            &[
                "archive_id",
                "slot",
                "agent",
                "task",
                "purpose",
                "machine",
                "generation",
                "created_at",
                "finished_at",
                "mode",
                "actor",
                "physical_storage",
                "validation",
                "limitations",
                "continuation",
                "checkouts",
            ],
            &["slot_type", "salvage", "layout", "import_source"],
            &label,
        )?;
        let archive_id = as_str(&mapping["archive_id"], &format!("{label}.archive_id"))?;
        if archive_ids.iter().any(|seen| seen == archive_id) {
            return Err(format!(
                "duplicate archive_id in {label_prefix}: {archive_id}"
            ));
        }
        archive_ids.push(archive_id.to_owned());
        let slot = validate_name(
            as_str(&mapping["slot"], &format!("{label}.slot"))?,
            "archived slot",
        )?;
        if archived_slots.contains(&slot) {
            return Err(format!("duplicate archived slot in {label_prefix}: {slot}"));
        }
        archived_slots.push(slot.clone());
        let agent = validate_name(
            as_str(&mapping["agent"], &format!("{label}.agent"))?,
            "archived agent",
        )?;
        let task = as_str(&mapping["task"], &format!("{label}.task"))?;
        let purpose = as_str(&mapping["purpose"], &format!("{label}.purpose"))?;
        if task.is_empty() || purpose.is_empty() {
            return Err(format!("{label} has an empty task or purpose"));
        }
        let slot_type = match mapping.get("slot_type") {
            None => "agent".to_owned(),
            Some(value) => as_str(value, &format!("{label}.slot_type"))?.to_owned(),
        };
        if !matches!(slot_type.as_str(), "agent" | "validate") {
            return Err(format!("{label}.slot_type must be one of agent, validate"));
        }
        let record_layout = match mapping.get("layout") {
            None => cfg.layout.clone(),
            Some(value) => validate_layout(as_str(value, &format!("{label}.layout"))?)?,
        };
        let import_source = import_from_obj(
            mapping.get("import_source"),
            &format!("{label}.import_source"),
        )?;
        if let Some(salvage) = mapping.get("salvage") {
            for (salvage_index, item) in as_list(salvage, &format!("{label}.salvage"))?
                .iter()
                .enumerate()
            {
                as_mapping(item, &format!("{label}.salvage[{salvage_index}]"))?;
            }
        }
        let record_machine = as_str(&mapping["machine"], &format!("{label}.machine"))?;
        if record_machine != selected {
            return Err(format!(
                "archived slot {slot} says machine {record_machine}, expected {selected}"
            ));
        }
        let generation = as_int(&mapping["generation"], &format!("{label}.generation"), 1)?;
        let created_at = as_str(&mapping["created_at"], &format!("{label}.created_at"))?;
        parse_timestamp(created_at, &label)?;
        let finished_at = as_str(&mapping["finished_at"], &format!("{label}.finished_at"))?;
        parse_timestamp(finished_at, &label)?;
        let expected_archive_id = format!("{selected}:{slot}:{generation}:{finished_at}");
        if archive_id != expected_archive_id {
            return Err(format!("{label}.archive_id does not match its record"));
        }
        let mode = as_str(&mapping["mode"], &format!("{label}.mode"))?;
        if mode != "remove" {
            return Err(format!("{label}.mode is invalid: {mode:?}"));
        }
        let actor = as_str(&mapping["actor"], &format!("{label}.actor"))?;
        if actor != "coordinator" {
            return Err(format!("{label}.actor does not match its operation"));
        }
        if as_str(
            &mapping["physical_storage"],
            &format!("{label}.physical_storage"),
        )? != "removed"
        {
            return Err(format!("{label} does not record removed physical storage"));
        }
        let validation = as_list(&mapping["validation"], &format!("{label}.validation"))?;
        if validation.is_empty() {
            return Err(format!("{label} has no validation evidence"));
        }
        for (evidence_index, evidence) in validation.iter().enumerate() {
            as_str(evidence, &format!("{label}.validation[{evidence_index}]"))?;
        }
        for (limitation_index, limitation) in
            as_list(&mapping["limitations"], &format!("{label}.limitations"))?
                .iter()
                .enumerate()
        {
            as_str(
                limitation,
                &format!("{label}.limitations[{limitation_index}]"),
            )?;
        }
        if as_str(&mapping["continuation"], &format!("{label}.continuation"))?.is_empty() {
            return Err(format!("{label} has no continuation"));
        }
        let mut checkouts = Vec::new();
        for (checkout_index, item) in as_list(&mapping["checkouts"], &format!("{label}.checkouts"))?
            .iter()
            .enumerate()
        {
            checkouts.push(checkout_from_obj(
                item,
                &format!("{label}.checkouts[{checkout_index}]"),
            )?);
        }
        let mut names: Vec<&str> = checkouts.iter().map(|item| item.name.as_str()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        if names.len() != total {
            return Err(format!("{label} has duplicate checkout names"));
        }
        if checkouts.is_empty() && (import_source.is_none() || record_layout != "nested") {
            return Err(format!("{label} has no checkouts"));
        }
        if record_layout == "flat" && checkouts.len() != 1 {
            return Err(format!("{label} has multiple checkouts under flat layout"));
        }
        if let Some(source) = &import_source {
            let row_label = format!("{label}.import_source.row");
            let source_owner = as_mapping(
                source.row.get("owner_sidecar").unwrap_or(&Value::Null),
                &format!("{row_label}.owner_sidecar"),
            )?;
            let (source_task, source_purpose) =
                historical_task_and_purpose(&source.row, &row_label, &agent)?;
            if record_layout != "nested"
                || source_task != task
                || source_purpose != purpose
                || source.row.get("allocated").and_then(Value::as_str) != Some(created_at)
                || source_owner.get("slot").and_then(Value::as_str) != Some(slot.as_str())
                || source_owner.get("agent").and_then(Value::as_str) != Some(agent.as_str())
            {
                return Err(format!("{label} differs from its historical source row"));
            }
        }
        for checkout in &checkouts {
            let slot_path = slot_directory(cfg, &slot, &slot_type)?;
            let destination = if record_layout == "flat" {
                slot_path
            } else {
                slot_path.join(&checkout.name)
            };
            let expected = posix_relative(&cfg.root, &destination)?;
            if checkout.path != expected {
                return Err(format!(
                    "{label} checkout {} path escaped its archived slot",
                    checkout.name
                ));
            }
            if checkout.landed_ref != landed_ref_for_remote(cfg, &checkout.remote)? {
                return Err(format!(
                    "{label} checkout {} differs from configured authority",
                    checkout.name
                ));
            }
        }
        records.push(item.clone());
    }
    let revision = as_int(&raw["revision"], "archive state.revision", 0)?;
    Ok(json!({
        "schema": SCHEMA,
        "machine": actual_machine,
        "revision": revision,
        "records": records,
    }))
}

/// Read and validate `ARCHIVED.<machine>.json` under a project's control
/// directory, printing the archive document with records as stored.
pub(crate) fn parse_archive(root: Option<&Path>, machine_override: Option<&str>) -> Result<Value> {
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    let path: PathBuf = cfg.control.join(format!("ARCHIVED.{selected}.json"));
    refuse_symlink(&path, "archive state")?;
    let text = std::fs::read_to_string(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("missing archive state: {}", path.display())
        } else {
            format!("cannot read archive state {}: {error}", path.display())
        }
    })?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("archive state is malformed JSON: {error}"))?;
    archive_from_obj(&cfg, &value, &selected, &path.to_string_lossy())
}
