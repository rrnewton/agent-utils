//! Doctor findings composition parity (snapshot scope).
//!
//! Composes the tolerant per-record status diagnosis with the
//! operational findings the existing doctor derives without authorizing
//! mutation: invalid holds, dead owners, unfinished journals,
//! interrupted atomic writes, and an active-slot cap breach. Registry
//! storage inconsistencies, slot-image attachment findings, and
//! event-derived owner releases stay with the existing `doctor`.
//! Read-only.

use std::path::Path;

use serde_json::{json, Value};

use crate::active_state::load_cfg;
use crate::config_validate::load_config;
use crate::journals_state::parse_journals;
use crate::status_state::status_records;

type Result<T> = std::result::Result<T, String>;

const REMEDY: &str = "inspect the named condition; diagnosis alone authorizes no mutation";

fn note(kind: &str, slot: Option<&str>, machine: Option<&str>, detail: String) -> Value {
    json!({
        "kind": kind,
        "scope": "operational",
        "slot": slot,
        "slot_type": Value::Null,
        "machine": machine,
        "checkout": Value::Null,
        "detail": detail,
        "remedy": REMEDY,
    })
}

fn glob_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            match rest.strip_prefix(part) {
                Some(stripped) => rest = stripped,
                None => return false,
            }
        } else if index == parts.len() - 1 {
            if !rest.ends_with(part) {
                return false;
            }
            rest = &rest[..rest.len() - part.len()];
        } else {
            match rest.find(part) {
                Some(position) => rest = &rest[position + part.len()..],
                None => return false,
            }
        }
    }
    true
}

fn partial_leftovers(control: &Path) -> Result<Vec<String>> {
    let patterns = [
        "ACTIVE.*.json.tmp.*",
        "ARCHIVED.*.json.tmp.*",
        "ACTIVE.*.journal.tmp.*",
        "CREATE.*.journal.tmp.*",
        "FINISH.*.journal.tmp.*",
    ];
    let entries = std::fs::read_dir(control).map_err(|error| {
        format!(
            "cannot list control directory {}: {error}",
            control.display()
        )
    })?;
    let mut names: Vec<(String, bool, std::path::PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot list control directory {}: {error}",
                control.display()
            )
        })?;
        let path = entry.path();
        let is_real_dir = path
            .symlink_metadata()
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        names.push((
            entry.file_name().to_string_lossy().into_owned(),
            is_real_dir,
            path,
        ));
    }
    let mut leftovers: Vec<String> = Vec::new();
    for pattern in patterns {
        let mut matched: Vec<String> = names
            .iter()
            .filter(|(name, _, _)| glob_match(pattern, name))
            .map(|(name, _, _)| name.clone())
            .collect();
        matched.sort();
        leftovers.extend(matched);
    }
    let mut event_dirs: Vec<&(String, bool, std::path::PathBuf)> = names
        .iter()
        .filter(|(name, is_dir, _)| name.starts_with("EVENTS.") && *is_dir)
        .collect();
    event_dirs.sort();
    for (_, _, directory) in event_dirs {
        let entries = std::fs::read_dir(directory)
            .map_err(|error| format!("cannot list event log {}: {error}", directory.display()))?;
        let mut matched: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!("cannot list event log {}: {error}", directory.display())
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if glob_match("*.json.tmp.*", &name) {
                matched.push(name);
            }
        }
        matched.sort();
        leftovers.extend(matched);
    }
    Ok(leftovers)
}

/// Diagnose the registry the way the existing doctor does: tolerant
/// status records plus operational findings. Scoped to the selected
/// machine unless `all_machines`, like the existing command.
pub(crate) fn parse_doctor(
    root: Option<&Path>,
    machine_override: Option<&str>,
    all_machines: bool,
) -> Result<Value> {
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    let cap = config
        .get("max_active_slots")
        .cloned()
        .unwrap_or(Value::Null);
    let (all_machines_list, all_records) = status_records(root, true)?;
    let (machines, records): (Vec<String>, Vec<Value>) = if all_machines {
        (all_machines_list, all_records)
    } else {
        (
            all_machines_list
                .into_iter()
                .filter(|machine| machine == &selected)
                .collect(),
            all_records
                .into_iter()
                .filter(|record| {
                    record.get("machine").and_then(Value::as_str) == Some(selected.as_str())
                })
                .collect(),
        )
    };
    let mut findings: Vec<Value> = Vec::new();
    for record in &records {
        let slot = record.get("slot").and_then(Value::as_str);
        let machine = record.get("machine").and_then(Value::as_str);
        if let Some(error) = record.get("hold_error").and_then(Value::as_str) {
            findings.push(note("invalid-hold", slot, machine, error.to_string()));
        }
        if record.get("owner_state").and_then(Value::as_str) == Some("dead") {
            let detail = record
                .get("owner_detail")
                .and_then(Value::as_str)
                .unwrap_or_default();
            findings.push(note(
                "dead-owner",
                slot,
                machine,
                format!(
                    "recorded owner is not running ({detail}); the slot is held by a row whose owner has exited"
                ),
            ));
        }
    }
    let journals = parse_journals(root)?;
    if let Some(names) = journals.get("journals").and_then(Value::as_array) {
        for name in names {
            if let Some(name) = name.as_str() {
                findings.push(note(
                    "unfinished-journal",
                    None,
                    None,
                    format!("recovery required: {name}"),
                ));
            }
        }
    }
    for row in crate::quarantine_write::quarantined_rows(&cfg) {
        let age = match row.get("age_seconds").and_then(Value::as_u64) {
            Some(seconds) => format!("{seconds}s"),
            None => "unknown".to_string(),
        };
        findings.push(note(
            "quarantined-journal",
            row.get("slot").and_then(Value::as_str),
            row.get("machine").and_then(Value::as_str),
            format!(
                "quarantined journal {} retained for inspection (age {age}); report-only in v1, hand restore is in the user guide",
                row.get("journal").and_then(Value::as_str).unwrap_or_default()
            ),
        ));
    }
    for name in partial_leftovers(&cfg.control)? {
        findings.push(note(
            "partial-atomic-update",
            None,
            None,
            format!("interrupted write left {name}; preserve it and run 'wrkslots recover'"),
        ));
    }
    if let Some(cap_value) = cap.as_i64() {
        let mine = records
            .iter()
            .filter(|record| record.get("machine").and_then(Value::as_str) == Some(&selected))
            .count();
        if mine as i64 > cap_value {
            findings.push(note(
                "cap-breach",
                None,
                Some(&selected),
                format!(
                    "{mine} active slot(s) against max_active_slots={cap_value} (excess {})",
                    mine as i64 - cap_value
                ),
            ));
        }
    }
    Ok(json!({
        "machines": machines,
        "active": records,
        "max_active_slots": cap,
        "findings": findings,
    }))
}

/// The Python `doctor` command's output, envelope and human form.
///
/// Unlike the scoped [`parse_doctor`], this composes every finding the
/// Python doctor reports — registry storage inconsistencies first,
/// then per-record operational notes including event-derived owner
/// releases, slot-image attachment findings, journals, interrupted
/// atomic writes, and the cap breach — and the full envelope
/// (`schema`, roots, `event_counts`).
pub(crate) fn doctor_cli(
    root: Option<&Path>,
    machine_override: Option<&str>,
    all_machines: bool,
    human: bool,
) -> Result<String> {
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    let cap = config
        .get("max_active_slots")
        .cloned()
        .unwrap_or(Value::Null);
    let (all_machine_names, records_all) = status_records(root, true)?;
    // The Python doctor validates the global state first (active and
    // archived overlap, duplicate slots or agents refuse), as
    // `status-text` does.
    crate::hold_write::validate_global(root, &cfg)?;
    let machines: Vec<String> = if all_machines {
        all_machine_names.clone()
    } else {
        all_machine_names
            .iter()
            .filter(|machine| *machine == &selected)
            .cloned()
            .collect()
    };
    // Event chains per machine: release lookups and the envelope's
    // event counts both read them, like the Python doctor.
    let mut events_by_machine: std::collections::HashMap<String, Vec<Value>> =
        std::collections::HashMap::new();
    for machine in &all_machine_names {
        let dir = cfg.control.join(format!("EVENTS.{machine}"));
        let events = if dir.is_dir() {
            crate::global_state::load_event_chain(&dir, machine)?
        } else {
            Vec::new()
        };
        events_by_machine.insert(machine.clone(), events);
    }
    // Release lookups are computed once per record; a malformed
    // release event refuses the doctor, as the Python edition's
    // release-state loading raises instead of skipping it.
    let mut releases: std::collections::HashMap<(String, String), Value> =
        std::collections::HashMap::new();
    for record in &records_all {
        let (Some(machine), Some(slot)) = (
            record.get("machine").and_then(Value::as_str),
            record.get("slot").and_then(Value::as_str),
        ) else {
            continue;
        };
        let events = events_by_machine
            .get(machine)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let (release, void) = crate::audit_state::release_state(&cfg, record, events)?;
        if let Some(release) = release {
            releases.insert(
                (machine.to_string(), slot.to_string()),
                crate::audit_state::release_obj(&release, void.as_deref()),
            );
        }
    }
    let release_for = |record: &Value| -> Option<Value> {
        let machine = record.get("machine").and_then(Value::as_str)?;
        let slot = record.get("slot").and_then(Value::as_str)?;
        releases
            .get(&(machine.to_string(), slot.to_string()))
            .cloned()
    };
    // Findings, in the Python doctor's composition order.
    let mut findings: Vec<Value> =
        crate::status_state::storage_inconsistencies(root, &records_all)?;
    for record in &records_all {
        let slot = record.get("slot").and_then(Value::as_str);
        let machine = record.get("machine").and_then(Value::as_str);
        if let Some(error) = record.get("hold_error").and_then(Value::as_str) {
            findings.push(note("invalid-hold", slot, machine, error.to_string()));
        }
        if record.get("owner_state").and_then(Value::as_str) == Some("dead") {
            let detail = record
                .get("owner_detail")
                .and_then(Value::as_str)
                .unwrap_or_default();
            findings.push(note(
                "dead-owner",
                slot,
                machine,
                format!(
                    "recorded owner is not running ({detail}); the slot is held by a row whose owner has exited"
                ),
            ));
        }
        if let Some(release) = release_for(record) {
            let state = release
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let summary = release
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let sequence = release
                .get("event_sequence")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if state == "released" {
                let generation = record
                    .get("generation")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                findings.push(note(
                    "owner-released",
                    slot,
                    machine,
                    format!(
                        "{summary} (event {sequence}, generation {generation}); remove no longer waits for the owner's exit or the heartbeat time-to-live, and still requires the handoff read and an unused slot"
                    ),
                ));
            } else {
                let recorded_at = release
                    .get("recorded_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let void_reason = release
                    .get("void_reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                findings.push(note(
                    "owner-release-void",
                    slot,
                    machine,
                    format!(
                        "release at {recorded_at} (event {sequence}) no longer applies: {void_reason}"
                    ),
                ));
            }
        }
    }
    findings.extend(identity_drift_findings(root, &cfg, &config, &selected)?);
    findings.extend(crate::storage_state::image_findings(&cfg, &selected));
    let journals = parse_journals(root)?;
    if let Some(names) = journals.get("journals").and_then(Value::as_array) {
        for name in names {
            if let Some(name) = name.as_str() {
                findings.push(note(
                    "unfinished-journal",
                    None,
                    None,
                    format!("recovery required: {name}"),
                ));
            }
        }
    }
    for row in crate::quarantine_write::quarantined_rows(&cfg) {
        let age = match row.get("age_seconds").and_then(Value::as_u64) {
            Some(seconds) => format!("{seconds}s"),
            None => "unknown".to_string(),
        };
        findings.push(note(
            "quarantined-journal",
            row.get("slot").and_then(Value::as_str),
            row.get("machine").and_then(Value::as_str),
            format!(
                "quarantined journal {} retained for inspection (age {age}); report-only in v1, hand restore is in the user guide",
                row.get("journal").and_then(Value::as_str).unwrap_or_default()
            ),
        ));
    }
    for name in partial_leftovers(&cfg.control)? {
        findings.push(note(
            "partial-atomic-update",
            None,
            None,
            format!("interrupted write left {name}; preserve it and run 'wrkslots recover'"),
        ));
    }
    if let Some(cap_value) = cap.as_i64() {
        let mine = records_all
            .iter()
            .filter(|record| {
                record.get("machine").and_then(Value::as_str) == Some(selected.as_str())
            })
            .count();
        if mine as i64 > cap_value {
            findings.push(note(
                "cap-breach",
                None,
                Some(&selected),
                format!(
                    "{mine} active slot(s) against max_active_slots={cap_value} (excess {})",
                    mine as i64 - cap_value
                ),
            ));
        }
    }
    if !all_machines {
        findings.retain(|item| {
            item.get("machine").is_none_or(|machine| {
                machine.is_null() || machine.as_str() == Some(selected.as_str())
            })
        });
    }
    let mut records: Vec<Value> = Vec::new();
    for mut record in records_all {
        if !machines
            .iter()
            .any(|machine| Some(machine.as_str()) == record.get("machine").and_then(Value::as_str))
        {
            continue;
        }
        if let Some(release) = release_for(&record) {
            if let Some(object) = record.as_object_mut() {
                object.insert("owner_release".to_string(), release);
            }
        }
        records.push(record);
    }
    if !human {
        let event_counts: serde_json::Map<String, Value> = machines
            .iter()
            .map(|machine| {
                (
                    machine.clone(),
                    json!(events_by_machine.get(machine).map_or(0, Vec::len)),
                )
            })
            .collect();
        let envelope = json!({
            "schema": 2,
            "project_root": cfg.root.to_string_lossy(),
            "worktrees_dir": cfg.worktrees.to_string_lossy(),
            "control_dir": cfg.control.to_string_lossy(),
            "layout": cfg.layout,
            "machines": machines,
            "event_counts": event_counts,
            "active": records,
            "max_active_slots": cap,
            "findings": findings,
        });
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&envelope)
                .map_err(|error| format!("cannot encode doctor: {error}"))?
        ));
    }
    let cap_text = if cap.is_null() {
        "none".to_string()
    } else {
        cap.to_string()
    };
    let mut out = format!(
        "project={} worktrees={} control={} layout={} active={} cap={} findings={}\n",
        cfg.root.display(),
        cfg.worktrees.display(),
        cfg.control.display(),
        cfg.layout,
        records.len(),
        cap_text,
        findings.len(),
    );
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for item in &findings {
        if let Some(kind) = item.get("kind").and_then(Value::as_str) {
            *counts.entry(kind.to_string()).or_default() += 1;
        }
    }
    for (kind, count) in &counts {
        out.push_str(&format!("  {count:4}  {kind}\n"));
    }
    for item in &findings {
        let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or_default();
        let slot = text("slot");
        out.push_str(&format!(
            "{}: {}: {}\n",
            text("kind"),
            if slot.is_empty() { "-" } else { slot },
            text("detail"),
        ));
    }
    Ok(out)
}

/// Bound on each `git config` read doctor makes for identity drift (#1097).
const IDENTITY_DRIFT_GIT_SECONDS: u64 = 10;

/// What Git reports for one setting in a checkout, as a user sees it.
enum EffectiveSetting {
    Set { origin: String, value: String },
    Unset,
    Unreadable(String),
}

/// The value of `key` in the checkout at `path`, every scope applying; see
/// the Python edition's `_effective_git_setting`.
fn effective_git_setting(path: &Path, key: &str) -> EffectiveSetting {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(path)
        .args(["config", "--show-origin", "--get", key])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, _value) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return EffectiveSetting::Unreadable(format!("cannot run git config: {error}"))
        }
    };
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(IDENTITY_DRIFT_GIT_SECONDS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return EffectiveSetting::Unreadable(format!(
                    "git config did not answer within {IDENTITY_DRIFT_GIT_SECONDS} seconds"
                ));
            }
            Err(error) => {
                return EffectiveSetting::Unreadable(format!("cannot run git config: {error}"))
            }
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let code = status.code().unwrap_or(-1);
    if code == 1 && stdout.is_empty() {
        return EffectiveSetting::Unset;
    }
    if code != 0 {
        let first = stderr.trim().lines().next().map(str::to_owned);
        return EffectiveSetting::Unreadable(match first {
            Some(line) => format!("git config exited {code}: {line}"),
            None => format!("git config exited {code}"),
        });
    }
    let line = stdout.strip_suffix('\n').unwrap_or(&stdout);
    let (origin, value) = line.split_once('\t').unwrap_or((line, ""));
    EffectiveSetting::Set {
        origin: origin.to_string(),
        value: value.to_string(),
    }
}

/// Checkouts of this machine's slots whose effective Git identity differs
/// from `configuration.identity` (#1097); see the Python edition's
/// `_identity_drift_findings`.  Advisory only.
fn identity_drift_findings(
    root: Option<&Path>,
    cfg: &crate::active_state::Cfg,
    config: &Value,
    selected: &str,
) -> Result<Vec<Value>> {
    let Some(identity) = config.get("identity").filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let expected = |key: &str| {
        identity
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let settings = [
        ("user.name", expected("name")),
        ("user.email", expected("email")),
    ];
    let active = crate::hold_write::active_document(root, cfg, selected)?;
    let mut findings = Vec::new();
    for record in active
        .get("slots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for checkout in record
            .get("checkouts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(raw) = checkout.get("path").and_then(Value::as_str) else {
                continue;
            };
            let Ok((_relative, path)) =
                crate::config_validate::relative_inside(&cfg.root, raw, "identity checkout path")
            else {
                continue;
            };
            if !path.is_dir() || path.is_symlink() {
                continue;
            }
            let mut drift = Vec::new();
            for (key, expected) in &settings {
                let wanted = crate::storage_state::py_repr(expected);
                match effective_git_setting(&path, key) {
                    EffectiveSetting::Unset => {
                        drift.push(format!("{key} is unset, configured {wanted}"))
                    }
                    EffectiveSetting::Unreadable(why) => {
                        drift.push(format!("{key} cannot be read ({why}), configured {wanted}"));
                    }
                    EffectiveSetting::Set { origin, value } if &value != expected => {
                        drift.push(format!(
                            "{key} is {} from {origin}, configured {wanted}",
                            crate::storage_state::py_repr(&value)
                        ))
                    }
                    EffectiveSetting::Set { .. } => {}
                }
            }
            if !drift.is_empty() {
                let name = checkout
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                findings.push(json!({
                    "kind": "identity-drift",
                    "scope": "row",
                    "slot": record.get("slot").cloned().unwrap_or(Value::Null),
                    "slot_type": record.get("slot_type").cloned().unwrap_or(Value::Null),
                    "machine": record.get("machine").cloned().unwrap_or(Value::Null),
                    "checkout": name,
                    "detail": format!("checkout {name}: {}", drift.join("; ")),
                    "remedy": "advisory: doctor never rewrites Git configuration; set user.name and user.email in this checkout's worktree configuration (git config --worktree), or keep an intended override",
                }));
            }
        }
    }
    Ok(findings)
}
