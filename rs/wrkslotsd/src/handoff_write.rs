//! Writer: publish one generation-bound handoff sidecar.
//!
//! Reproduces the existing `write-handoff` mutation: bounded regular
//! source capture bound to its file identity, canonical-source and
//! outside-Git checks, owner authorization, the intent/completion
//! publication events, the no-replace sidecar write, and the final
//! `handoff-written` event. Fixture registries only until the
//! edition cutover (see `ensure_states::require_fixture`).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{load_cfg, slot_directory};
use crate::config_validate::validate_name;
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state, require_fixture,
    utc_now,
};
use crate::global_state::{event_log_status, load_event_chain, EventLogStatus};
use crate::handoff_state::{
    identity_to_obj, read_regular_file_identity, sidecar_path, FileIdentity,
};
use crate::hold_write::{acquire_mutation_locks, active_document, validate_global};
use crate::journals_state::parse_journals;
use crate::status_state::{process_state, read_process_identity, Identity};

type Result<T> = std::result::Result<T, String>;

const LIMIT: usize = 1024 * 1024;

fn git_toplevel(directory: &Path) -> Result<Option<PathBuf>> {
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "core.fsmonitor=false",
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
        ])
        .current_dir(directory)
        .env_clear();
    for (key, value) in std::env::vars() {
        if !key.starts_with("GIT_") {
            command.env(key, value);
        }
    }
    let output = command
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| format!("cannot determine Git provenance: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        if !stderr.is_empty() || stdout.is_empty() || stdout.contains('\n') {
            return Err("Git returned an invalid worktree root".to_string());
        }
        let root = PathBuf::from(&stdout);
        let resolved = std::fs::canonicalize(&root)
            .map_err(|error| format!("cannot determine Git provenance: {error}"))?;
        if !root.is_absolute() || root != resolved || !root.is_dir() {
            return Err(format!(
                "Git returned an unsafe worktree root {}",
                root.display()
            ));
        }
        return Ok(Some(root));
    }
    if output.status.code() == Some(128) && stderr.starts_with("fatal: not a git repository") {
        return Ok(None);
    }
    Err(format!(
        "cannot determine Git provenance: git rev-parse failed: {stderr}"
    ))
}

fn ancestor_git_marker(source: &Path) -> Result<Option<PathBuf>> {
    let mut current = Some(source.parent().unwrap_or(Path::new(".")));
    while let Some(directory) = current {
        let marker = directory.join(".git");
        match marker.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "cannot determine Git provenance for handoff input {}: ancestor marker is a symlink: {}",
                    source.display(),
                    marker.display()
                ));
            }
            Ok(meta) if !meta.is_file() && !meta.is_dir() => {
                return Err(format!(
                    "cannot determine Git provenance for handoff input {}: ancestor marker is not a regular file or directory: {}",
                    source.display(),
                    marker.display()
                ));
            }
            Ok(_) => return Ok(Some(marker)),
            Err(_) => {}
        }
        current = directory.parent();
    }
    Ok(None)
}

fn assert_external(source: &Path) -> Result<()> {
    let parent = source.parent().unwrap_or(Path::new("."));
    let marker = ancestor_git_marker(source)?;
    let root = git_toplevel(parent)?;
    if root.is_none() && marker.is_some() {
        return Err(format!(
            "cannot determine Git provenance for handoff input {}: Git rejected the ancestor marker",
            source.display()
        ));
    }
    if let Some(root) = root {
        return Err(format!(
            "handoff input is inside Git working tree {}: {}; copy intentionally authored handoff bytes to an external regular file and retry",
            root.display(),
            source.display()
        ));
    }
    Ok(())
}

fn canonical_source(path: &Path, contents: &[u8], identity: &FileIdentity) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve handoff input {}: {error}", path.display()))?;
    let (again, again_identity) = read_regular_file_identity(&canonical, "handoff input", LIMIT)?;
    if again != contents
        || again_identity.device != identity.device
        || again_identity.inode != identity.inode
        || again_identity.size != identity.size
        || again_identity.sha256 != identity.sha256
    {
        return Err(format!(
            "handoff input changed while its canonical path was resolved: {}",
            path.display()
        ));
    }
    Ok(canonical)
}

#[allow(clippy::too_many_arguments)]
fn artifact_obj(
    machine: &str,
    slot: &str,
    generation: i64,
    recorded_at: &str,
    source_path: &Path,
    identity: &FileIdentity,
    contents: &[u8],
    foreign: &[(String, String)],
) -> Value {
    let mut value = json!({
        "schema": 2,
        "machine": machine,
        "slot": slot,
        "generation": generation,
        "recorded_at": recorded_at,
        "source": "write-handoff",
        "source_path": source_path.to_string_lossy(),
        "source_identity": identity_to_obj(identity),
        "contents_utf8": String::from_utf8_lossy(contents),
        "sha256": format!("{:x}", Sha256::digest(contents)),
    });
    if !foreign.is_empty() {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "foreign_checkouts".to_string(),
                json!(foreign
                    .iter()
                    .map(|(repository, path)| json!({
                        "repository": repository,
                        "path": path,
                    }))
                    .collect::<Vec<_>>()),
            );
        }
    }
    value
}

fn validate_intent(payload: &Value, slot: &str, generation: i64) -> Result<()> {
    let artifact = payload.get("artifact").cloned().unwrap_or(Value::Null);
    let mut keys: Vec<&str> = artifact
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort();
    let mut required = vec![
        "contents_utf8",
        "generation",
        "machine",
        "recorded_at",
        "schema",
        "sha256",
        "slot",
        "source",
        "source_identity",
        "source_path",
    ];
    required.sort();
    if keys != required
        && keys != {
            let mut with_foreign = required.clone();
            with_foreign.push("foreign_checkouts");
            with_foreign.sort();
            with_foreign
        }
    {
        return Err("handoff write intent has an invalid artifact identity".to_string());
    }
    if artifact.get("schema").and_then(Value::as_i64) != Some(2)
        || artifact.get("slot").and_then(Value::as_str) != Some(slot)
        || artifact.get("generation").and_then(Value::as_i64) != Some(generation)
        || artifact.get("source").and_then(Value::as_str) != Some("write-handoff")
    {
        return Err("handoff write intent has an invalid artifact identity".to_string());
    }
    let recorded = artifact
        .get("recorded_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if chrono::DateTime::parse_from_rfc3339(recorded).is_err() {
        return Err("handoff write intent recorded_at is invalid".to_string());
    }
    Ok(())
}

fn handoff_owner_step(
    owner: Option<Value>,
    event: &Value,
    machine: &str,
    slot: &str,
    generation: i64,
) -> Option<Value> {
    let payload = event.get("payload").cloned().unwrap_or(Value::Null);
    match event.get("kind").and_then(Value::as_str) {
        Some("state-imported") => payload
            .pointer("/active/slots")
            .and_then(Value::as_array)
            .and_then(|slots| {
                slots.iter().find(|item| {
                    item.get("machine").and_then(Value::as_str) == Some(machine)
                        && item.get("slot").and_then(Value::as_str) == Some(slot)
                        && item.get("generation").and_then(Value::as_i64) == Some(generation)
                })
            })
            .and_then(|record| record.get("owner").cloned()),
        Some("active-state-recorded") => {
            if payload.get("slot").and_then(Value::as_str) != Some(slot) {
                return owner;
            }
            let record = payload.get("record").filter(|value| !value.is_null())?;
            if record.get("machine").and_then(Value::as_str) == Some(machine)
                && record.get("slot").and_then(Value::as_str) == Some(slot)
                && record.get("generation").and_then(Value::as_i64) == Some(generation)
            {
                record.get("owner").cloned()
            } else {
                None
            }
        }
        _ => owner,
    }
}

fn writer_matches_owner(owner: &Option<Value>, writer: Option<&Value>) -> bool {
    let (Some(owner), Some(writer)) = (owner, writer) else {
        return false;
    };
    let Some(owner_identity) = crate::status_state::identity_from_value(owner) else {
        return false;
    };
    crate::status_state::identity_matches_value(&owner_identity, writer)
}

pub(crate) fn owner_before_intent(
    events: &[Value],
    machine: &str,
    slot: &str,
    generation: i64,
    intent_sequence: i64,
) -> Option<Value> {
    let mut owner: Option<Value> = None;
    for event in events {
        let sequence = event.get("sequence").and_then(Value::as_i64).unwrap_or(0);
        if sequence >= intent_sequence {
            break;
        }
        owner = handoff_owner_step(owner, event, machine, slot, generation);
    }
    owner
}

/// Validate the latest publication intent the way the existing
/// publication fold does: artifact identity, writer binding to the
/// owner recorded at intent time, known-intent completions, and
/// completion digests.
pub(crate) fn validate_publication(
    events: &[Value],
    machine: &str,
    slot: &str,
    generation: i64,
) -> Result<()> {
    let mut intent_sequences: Vec<i64> = Vec::new();
    let mut owner_at_intent: Option<Value> = None;
    for event in events {
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        match event.get("kind").and_then(Value::as_str) {
            Some("state-imported") | Some("active-state-recorded") => {
                owner_at_intent =
                    handoff_owner_step(owner_at_intent.clone(), event, machine, slot, generation);
            }
            _ => {}
        }
        if payload.get("slot").and_then(Value::as_str) != Some(slot)
            || payload.get("generation").and_then(Value::as_i64) != Some(generation)
        {
            continue;
        }
        match event.get("kind").and_then(Value::as_str) {
            Some("handoff-write-intended") => {
                validate_intent(&payload, slot, generation)?;
                if !writer_matches_owner(&owner_at_intent, payload.get("writer")) {
                    return Err(
                        "handoff write intent writer does not match the recorded slot owner"
                            .to_string(),
                    );
                }
                if let Some(sequence) = event.get("sequence").and_then(Value::as_i64) {
                    intent_sequences.push(sequence);
                }
            }
            Some("handoff-write-completed") => {
                let sequence = payload
                    .get("intent_sequence")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                if !intent_sequences.contains(&sequence) {
                    return Err("handoff write completion references an unknown intent".to_string());
                }
                let intended = events
                    .iter()
                    .find(|candidate| {
                        candidate.get("sequence").and_then(Value::as_i64) == Some(sequence)
                    })
                    .and_then(|candidate| candidate.get("payload"))
                    .and_then(|payload| payload.get("artifact"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let expected = crate::canonical::canonical_sha256(&intended)
                    .map_err(|error| error.to_string())?;
                if payload.get("artifact_sha256").and_then(Value::as_str) != Some(expected.as_str())
                {
                    return Err(
                        "handoff write completion digest does not match its intent".to_string()
                    );
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn publication(
    events: &[Value],
    slot: &str,
    generation: i64,
) -> Option<(i64, Value, bool)> {
    let mut latest: Option<(i64, Value)> = None;
    let mut completed: Vec<i64> = Vec::new();
    for event in events {
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if payload.get("slot").and_then(Value::as_str) != Some(slot)
            || payload.get("generation").and_then(Value::as_i64) != Some(generation)
        {
            continue;
        }
        match event.get("kind").and_then(Value::as_str) {
            Some("handoff-write-intended") => {
                if let Some(sequence) = event.get("sequence").and_then(Value::as_i64) {
                    if latest
                        .as_ref()
                        .map(|(seq, _)| sequence >= *seq)
                        .unwrap_or(true)
                    {
                        latest = Some((
                            sequence,
                            payload.get("artifact").cloned().unwrap_or(Value::Null),
                        ));
                    }
                }
            }
            Some("handoff-write-completed") => {
                if let Some(sequence) = payload.get("intent_sequence").and_then(Value::as_i64) {
                    completed.push(sequence);
                }
            }
            _ => {}
        }
    }
    latest.map(|(sequence, artifact)| (sequence, artifact, completed.contains(&sequence)))
}

pub(crate) fn write_noreplace(path: &Path, payload: &Value) -> Result<()> {
    if path.symlink_metadata().is_ok() {
        return Err(format!(
            "handoff sidecar already exists and is immutable: {}",
            path.display()
        ));
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create handoff sidecar {}: {error}", path.display()))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prefix = format!("{name}.tmp.");
    let mut leftovers: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                leftovers.push(entry.path());
            }
        }
    }
    leftovers.sort();
    if let Some(first) = leftovers.first() {
        return Err(format!(
            "partial handoff sidecar publication exists: {}; inspect it and use 'wrkslots recover'",
            first.display()
        ));
    }
    let mut text = serde_json::to_string_pretty(payload).map_err(|error| error.to_string())?;
    text.push('\n');
    let temp = parent.join(format!(
        "{prefix}{}.{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("cannot write handoff sidecar {}: {error}", path.display()))?;
        file.write_all(crate::ensure_states::escape_ascii_json(&text).as_bytes())
            .map_err(|error| format!("cannot write handoff sidecar {}: {error}", path.display()))?;
        file.sync_all()
            .map_err(|error| format!("cannot write handoff sidecar {}: {error}", path.display()))?;
    }
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    if let Err(error) = rustix::fs::renameat_with(
        rustix::fs::CWD,
        temp.as_path(),
        rustix::fs::CWD,
        path,
        rustix::fs::RenameFlags::NOREPLACE,
    ) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "handoff sidecar already exists and is immutable: {}: {error}",
            path.display()
        ));
    }
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

pub(crate) fn committed_project_file(path: &Path) -> bool {
    let parent = path.parent().unwrap_or(Path::new("."));
    let Ok(Some(root)) = git_toplevel(parent) else {
        return false;
    };
    let Ok(relative) = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .strip_prefix(&root)
        .map(|rel| rel.to_string_lossy().into_owned())
    else {
        return false;
    };
    let run = |args: &[&str]| -> Option<String> {
        let mut command = Command::new("git");
        command
            .args(["-C", &root.to_string_lossy()])
            .args(args)
            .env_clear();
        for (key, value) in std::env::vars() {
            if !key.starts_with("GIT_") {
                command.env(key, value);
            }
        }
        let output = command
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .ok()?;
        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        }
    };
    if run(&["ls-files", "--error-unmatch", "--", &relative]).is_none() {
        return false;
    }
    matches!(
        run(&["status", "--porcelain", "--", &relative]).as_deref(),
        Some("")
    )
}

fn identity_value(identity: &Identity) -> Value {
    crate::status_state::identity_to_value(identity)
}

fn parent_pid(pid: i64) -> Option<i64> {
    let raw = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let closing = text.rfind(") ")?;
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    fields.get(1).and_then(|value| value.parse().ok())
}

fn assert_caller(identity: &Identity) -> Result<()> {
    let mut current: Option<i64> = Some(std::process::id() as i64);
    let mut seen: Vec<i64> = Vec::new();
    for _ in 0..256 {
        let Some(pid) = current else { break };
        if seen.contains(&pid) {
            break;
        }
        seen.push(pid);
        if let Ok(candidate) = read_process_identity(pid) {
            if candidate == *identity {
                return Ok(());
            }
        }
        current = parent_pid(pid);
    }
    Err(format!(
        "owner PID {} is not in the invoking process ancestry",
        identity.pid
    ))
}

/// Publish the handoff for the named slot generation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_handoff(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    agent: &str,
    owner_pid: i64,
    expected_generation: i64,
    from_file: &Path,
    supersede_own_checkpoint: bool,
    reason: Option<&str>,
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
    if supersede_own_checkpoint && reason.map(|text| text.trim().is_empty()).unwrap_or(true) {
        return Err(
            "write-handoff --supersede-own-checkpoint requires a non-empty --reason".to_string(),
        );
    }
    let supplied = read_process_identity(owner_pid)?;
    assert_caller(&supplied)?;
    let requested = if from_file.is_absolute() {
        from_file.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(from_file)
    };
    let (contents, source_identity) =
        read_regular_file_identity(&requested, "handoff input", LIMIT)?;
    if std::str::from_utf8(&contents).is_err() {
        return Err(format!(
            "handoff input is not UTF-8: {}",
            requested.display()
        ));
    }
    let source = canonical_source(&requested, &contents, &source_identity)?;
    assert_external(&source)?;
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    crate::hold_write::report_dead_findings(root, slot)?;
    let journals = parse_journals(root)?;
    if let Some(names) = journals.get("journals").and_then(Value::as_array) {
        if let Some(first) = names.first().and_then(Value::as_str) {
            return Err(crate::journals_state::journal_blocker_message(
                &cfg.control,
                first,
            ));
        }
    }
    validate_global(root, &cfg)?;
    let active = active_document(root, &cfg, &selected)?;
    let record = active
        .get("slots")
        .and_then(Value::as_array)
        .and_then(|slots| {
            slots
                .iter()
                .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
        })
        .cloned()
        .ok_or_else(|| format!("slot '{slot}' is not active on machine {selected}"))?;
    if record.get("slot_type").and_then(Value::as_str) != Some("agent") {
        return Err("validation slots cannot carry agent handoffs".to_string());
    }
    let mut generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if generation != expected_generation {
        return Err(format!(
            "stale generation for slot {slot}: expected {expected_generation}, current {generation}"
        ));
    }
    let record_agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if record_agent != agent {
        return Err(format!(
            "ownership mismatch for slot {slot}: expected {record_agent}, got {agent}"
        ));
    }
    let owner_value = record.get("owner").cloned().unwrap_or(Value::Null);
    if owner_value.is_null() {
        return Err(format!(
            "slot {slot} has no bound owner process; adopt it first"
        ));
    }
    if !crate::status_state::identity_matches_value(&supplied, &owner_value) {
        let owner = Identity {
            pid: owner_value.get("pid").and_then(Value::as_i64).unwrap_or(0),
            start_ticks: owner_value
                .get("start_ticks")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            boot_id: owner_value
                .get("boot_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            host_id: owner_value
                .get("host_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            cgroup_path: owner_value
                .get("cgroup_path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            namespace_pid: crate::status_state::optional_namespace_pid(&owner_value),
            pidns_inode: crate::status_state::optional_pidns_inode(&owner_value),
        };
        let (state, detail) = process_state(Some(&owner));
        return Err(format!(
            "owner process generation mismatch for slot {slot}; recorded owner is {state}: {detail}"
        ));
    }
    assert_caller(&supplied)?;
    // A recorded finish (#1868) does not block publishing the sidecar:
    // finish records its evidence in the record, while the sidecar is
    // the owner's separate notes artifact that release and
    // owner-consented removal consume. An existing sidecar is still
    // refused below unless byte-identical, so published handoffs stay
    // immutable; the finish record itself is not modified here.
    // An undeclared nested linked worktree of another repository does
    // not stop a handoff (#1400): warn loudly and record it in the
    // sidecar; remove and retire still refuse it.
    let foreign = crate::hold_write::foreign_checkouts(&cfg, &active, &record);
    crate::hold_write::refuse_target_storage_tolerating_foreign(root, slot, &foreign)?;
    for (repository, path) in &foreign {
        eprintln!(
            "WARNING: slot {slot} holds an undeclared nested checkout of repository {repository} at {path}; the handoff records it, and removal still refuses until it is declared with 'wrkslots adopt-checkout' or moved out with 'git worktree move'"
        );
    }
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let mut path = sidecar_path(&cfg.control, &selected, slot, generation);
    let mut existing_schema1 = false;
    let mut existing: Option<Value> = if path.exists() || path.is_symlink() {
        let (bytes, _) = read_regular_file_identity(&path, "handoff sidecar", LIMIT)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("handoff sidecar {} is invalid: not UTF-8", path.display()))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("handoff sidecar {} is invalid: {error}", path.display()))?;
        let schema = value.get("schema").and_then(Value::as_i64).unwrap_or(0);
        let mut keys: Vec<&str> = value
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort();
        let expected: &[&str] = if schema == 2 {
            &[
                "contents_utf8",
                "generation",
                "machine",
                "recorded_at",
                "schema",
                "sha256",
                "slot",
                "source",
                "source_identity",
                "source_path",
            ]
        } else {
            &[
                "contents_utf8",
                "generation",
                "machine",
                "recorded_at",
                "schema",
                "sha256",
                "slot",
                "source",
                "source_identity",
            ]
        };
        let keys_match = keys == expected
            || (schema == 2
                && keys.len() == expected.len() + 1
                && keys.contains(&"foreign_checkouts")
                && expected.iter().all(|key| keys.contains(key)));
        if !keys_match {
            return Err(format!(
                "handoff sidecar {} has the wrong keys",
                path.display()
            ));
        }
        if value.get("machine").and_then(Value::as_str) != Some(selected.as_str())
            || value.get("slot").and_then(Value::as_str) != Some(slot)
            || value.get("generation").and_then(Value::as_i64) != Some(generation)
        {
            return Err(format!(
                "handoff sidecar identity does not match slot generation: {}",
                path.display()
            ));
        }
        let contents_text = value
            .get("contents_utf8")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let digest = format!("{:x}", Sha256::digest(contents_text.as_bytes()));
        if value.get("sha256").and_then(Value::as_str) != Some(digest.as_str()) {
            return Err(format!(
                "handoff sidecar {} digest does not match its content",
                path.display()
            ));
        }
        if schema == 1 {
            existing_schema1 = true;
        } else if schema != 2 {
            return Err(format!(
                "unsupported handoff sidecar schema in {}",
                path.display()
            ));
        }
        Some(value)
    } else {
        None
    };
    let slot_dir = slot_directory(&cfg, slot, slot_type)?;
    let legacy_path = slot_dir.join("HANDOFF.md");
    if legacy_path.exists() || legacy_path.is_symlink() {
        let (legacy_contents, _) =
            read_regular_file_identity(&legacy_path, "slot HANDOFF.md", LIMIT)?;
        let committed = committed_project_file(&legacy_path);
        if !committed && legacy_contents != contents {
            return Err(format!(
                "slot {slot} legacy HANDOFF.md disagrees with the proposed sidecar; preserve both versions and reconcile them before writing"
            ));
        }
    }
    if existing_schema1 {
        return Err(format!(
            "slot {slot} handoff sidecar schema 1 lacks auditable source provenance and is permanently manual-only; preserve it for inspection"
        ));
    }
    ensure_event_log(root, &cfg, &selected)?;
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    let mut events = if let EventLogStatus::Derived = event_log_status(&directory) {
        load_event_chain(&directory, &selected)?
    } else {
        Vec::new()
    };
    let mut published = publication(&events, slot, generation);
    if let Some((sequence, intended, completed)) = &published {
        let intent_event = events
            .iter()
            .find(|event| event.get("sequence").and_then(Value::as_i64) == Some(*sequence));
        if let Some(event) = intent_event {
            let payload = event.get("payload").cloned().unwrap_or(Value::Null);
            validate_intent(&payload, slot, generation)?;
            let owner_at_intent =
                owner_before_intent(&events, &selected, slot, generation, *sequence);
            if !writer_matches_owner(&owner_at_intent, payload.get("writer")) {
                return Err(
                    "handoff write intent writer does not match the recorded slot owner"
                        .to_string(),
                );
            }
        }
        if *completed {
            let digest_ok = events.iter().any(|event| {
                event.get("kind").and_then(Value::as_str) == Some("handoff-write-completed")
                    && event
                        .get("payload")
                        .and_then(|payload| payload.get("intent_sequence"))
                        .and_then(Value::as_i64)
                        == Some(*sequence)
                    && event
                        .get("payload")
                        .and_then(|payload| payload.get("artifact_sha256"))
                        .and_then(Value::as_str)
                        .map(|digest| {
                            crate::canonical::canonical_sha256(intended)
                                .map(|expected| expected == digest)
                                .unwrap_or(false)
                        })
                        .unwrap_or(false)
            });
            if !digest_ok {
                return Err("handoff write completion digest does not match its intent".to_string());
            }
        }
    }
    if supersede_own_checkpoint {
        if let Some(current) = existing.clone() {
            // A flagged re-write byte-identical to the completed sidecar
            // (same provenance as an unflagged identical re-write)
            // completes in place: no generation bump and no new events.
            // This precedes the read/release gates because an unflagged
            // identical re-write also completes after a read; different
            // bytes keep every supersede gate below unchanged.
            let identical_digest = format!("{:x}", Sha256::digest(&contents));
            let source_text = source.to_string_lossy().into_owned();
            let is_identical = current
                .get("contents_utf8")
                .and_then(Value::as_str)
                .map(|text| text.as_bytes())
                == Some(contents.as_slice())
                && current.get("sha256").and_then(Value::as_str) == Some(identical_digest.as_str())
                && current.get("source").and_then(Value::as_str) == Some("write-handoff")
                && current.get("source_path").and_then(Value::as_str) == Some(source_text.as_str())
                && current.get("source_identity") == Some(&identity_to_obj(&source_identity));
            let is_completed_identical = is_identical
                && matches!(&published, Some((_, intended, true)) if *intended == current);
            if is_completed_identical {
                return Ok(json!({
                    "machine": selected,
                    "slot": slot,
                    "generation": generation,
                    "sha256": identical_digest,
                    "path": path.to_string_lossy(),
                }));
            }
            // Owner-authenticated supersede of one's own checkpoint
            // (#1975 phase 6, option (a)): only a completed schema-2
            // write-handoff sidecar that has been neither read for
            // retirement nor released may give way.
            if current.get("source").and_then(Value::as_str) != Some("write-handoff") {
                return Err(format!(
                    "slot {slot} already has an immutable handoff sidecar with different contents or source provenance; preserve both versions for manual review"
                ));
            }
            let (intent_sequence, _intended, completed) = match &published {
                Some((sequence, intended, done)) if *intended == current => {
                    (*sequence, intended.clone(), *done)
                }
                _ => {
                    return Err(format!(
                        "slot {slot} existing handoff sidecar has no matching owner-authenticated publication intent; preserve it for manual review"
                    ));
                }
            };
            if !completed {
                return Err(format!(
                    "slot {slot} handoff publication is incomplete; preserve the sidecar and have its exact owner retry write-handoff"
                ));
            }
            let superseded_sha = current
                .get("sha256")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let latest_read =
                crate::retirement_queue::latest_handoff_read(&events, slot, generation)?;
            if latest_read.map(|read| read.enqueued).unwrap_or(false) {
                return Err(format!(
                    "slot {slot} handoff sidecar is immutable once read or released; it has already been read for retirement, so its owner cannot supersede it. remedy: have the coordinator supersede the read with 'wrkslots supersede-handoff' if the lane keeps using the slot"
                ));
            }
            if crate::heartbeat_write::release_in_effect(&cfg, &record, &events)?.is_some() {
                return Err(format!(
                    "slot {slot} handoff sidecar is immutable once read or released; this generation has already been released by its owner, so it cannot be superseded. remedy: preserve the released handoff for coordinator retirement"
                ));
            }
            let reason_text = reason.unwrap_or_default().to_string();
            let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
            let previous_digest =
                crate::canonical::canonical_sha256(&record).map_err(|error| error.to_string())?;
            let mut bumped = record.clone();
            if let Some(object) = bumped.as_object_mut() {
                object.insert("generation".to_string(), json!(generation + 1));
            }
            append_event(
                &cfg,
                &selected,
                "active-state-recorded",
                json!({
                    "action": "handoff-checkpoint-superseded",
                    "slot": slot,
                    "previous_revision": previous_revision,
                    "revision": previous_revision + 1,
                    "previous_record_sha256": previous_digest,
                    "record": bumped,
                    "evidence": {
                        "writer": identity_value(&supplied),
                        "previous_generation": generation,
                        "superseded_sidecar_sha256": superseded_sha,
                        "superseded_intent_sequence": intent_sequence,
                        "reason": reason_text,
                    },
                }),
            )?;
            let mut slots: Vec<Value> = active
                .get("slots")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for item in slots.iter_mut() {
                if item.get("slot").and_then(Value::as_str) == Some(slot) {
                    *item = bumped.clone();
                }
            }
            let mut document = active.clone();
            if let Some(object) = document.as_object_mut() {
                object.insert("revision".to_string(), json!(previous_revision + 1));
                object.insert("slots".to_string(), json!(slots));
            }
            let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
            atomic_write_json(&active_path, &document)?;
            validate_global(root, &cfg)?;
            generation += 1;
            path = sidecar_path(&cfg.control, &selected, slot, generation);
            existing = None;
            events = load_event_chain(&directory, &selected)?;
            published = publication(&events, slot, generation);
        }
    }
    let digest = format!("{:x}", Sha256::digest(&contents));
    let mut artifact = artifact_obj(
        &selected,
        slot,
        generation,
        &utc_now(),
        &source,
        &source_identity,
        &contents,
        &foreign,
    );
    let intent_sequence: i64;
    if existing.is_none() {
        match published {
            None => {
                let intent = append_event(
                    &cfg,
                    &selected,
                    "handoff-write-intended",
                    json!({
                        "slot": slot,
                        "generation": generation,
                        "artifact": artifact,
                        "writer": identity_value(&supplied),
                    }),
                )?;
                intent_sequence = intent.get("sequence").and_then(Value::as_i64).unwrap_or(0);
            }
            Some((sequence, intended, completed)) => {
                if completed {
                    return Err(format!(
                        "slot {slot} completed handoff sidecar disappeared; preserve the slot and repair its exact artifact"
                    ));
                }
                let recorded = intended
                    .get("recorded_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                artifact = artifact_obj(
                    &selected,
                    slot,
                    generation,
                    &recorded,
                    &source,
                    &source_identity,
                    &contents,
                    &foreign,
                );
                if intended != artifact {
                    return Err(format!(
                        "slot {slot} has an outstanding handoff intent for different bytes or source provenance; preserve it for recovery"
                    ));
                }
                intent_sequence = sequence;
            }
        }
        write_noreplace(&path, &artifact)?;
        let (written_bytes, _) = read_regular_file_identity(&path, "handoff sidecar", LIMIT)?;
        let written: Value = serde_json::from_slice(&written_bytes)
            .map_err(|error| format!("handoff sidecar {} is invalid: {error}", path.display()))?;
        if written != artifact {
            return Err(
                "handoff sidecar differs from the owner's exact published artifact; preserve it and the external source for recovery"
                    .to_string(),
            );
        }
    } else {
        let current = existing.clone().unwrap_or(Value::Null);
        if current
            .get("contents_utf8")
            .and_then(Value::as_str)
            .map(|text| text.as_bytes())
            != Some(contents.as_slice())
            || current.get("sha256").and_then(Value::as_str) != Some(digest.as_str())
            || current.get("source").and_then(Value::as_str) != Some("write-handoff")
            || current.get("source_path").and_then(Value::as_str)
                != Some(source.to_string_lossy().as_ref())
            || current.get("source_identity") != Some(&identity_to_obj(&source_identity))
        {
            return Err(format!(
                "slot {slot} already has an immutable handoff sidecar with different contents or source provenance; preserve both versions for manual review"
            ));
        }
        artifact = current;
        match published {
            Some((sequence, intended, _)) if intended == artifact => {
                intent_sequence = sequence;
            }
            _ => {
                return Err(format!(
                    "slot {slot} existing handoff sidecar has no matching owner-authenticated publication intent; preserve it for manual review"
                ));
            }
        }
    }
    // Complete the publication, then record the write.
    let events = load_event_chain(&directory, &selected)?;
    let (latest_sequence, latest_artifact, completed) = publication(&events, slot, generation)
        .ok_or_else(|| "handoff write intent changed before completion".to_string())?;
    if latest_sequence != intent_sequence || latest_artifact != artifact {
        return Err("handoff write intent changed before completion".to_string());
    }
    if !completed {
        append_event(
            &cfg,
            &selected,
            "handoff-write-completed",
            json!({
                "slot": slot,
                "generation": generation,
                "intent_sequence": intent_sequence,
                "artifact_sha256": crate::canonical::canonical_sha256(&artifact)
                    .map_err(|error| error.to_string())?,
            }),
        )?;
    }
    append_event(
        &cfg,
        &selected,
        "handoff-written",
        json!({
            "slot": slot,
            "generation": generation,
            "sha256": digest,
            "path": path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
            "source": "write-handoff",
            "source_path": source.to_string_lossy(),
            "source_identity": identity_to_obj(&source_identity),
            "writer": identity_value(&supplied),
        }),
    )?;
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "generation": generation,
        "sha256": digest,
        "path": path.to_string_lossy(),
    }))
}
