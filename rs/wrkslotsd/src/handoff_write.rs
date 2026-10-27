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
    append_event, ensure_event_log, refuse_partial_state, require_fixture, utc_now,
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

fn artifact_obj(
    machine: &str,
    slot: &str,
    generation: i64,
    recorded_at: &str,
    source_path: &Path,
    identity: &FileIdentity,
    contents: &[u8],
) -> Value {
    json!({
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
    })
}

fn validate_intent(payload: &Value, slot: &str, generation: i64) -> Result<()> {
    let artifact = payload.get("artifact").cloned().unwrap_or(Value::Null);
    let mut keys: Vec<&str> = artifact
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort();
    if keys
        != [
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

fn publication(events: &[Value], slot: &str, generation: i64) -> Option<(i64, Value, bool)> {
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

fn write_noreplace(path: &Path, payload: &Value) -> Result<()> {
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

fn committed_project_file(path: &Path) -> bool {
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
    json!({
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    })
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
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
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
    let journals = parse_journals(root)?;
    if let Some(names) = journals.get("journals").and_then(Value::as_array) {
        if let Some(first) = names.first().and_then(Value::as_str) {
            return Err(format!(
                "interrupted mutation recorded in {}; run 'wrkslots recover' first",
                cfg.control.join(first).display()
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
    let generation = record
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
    if identity_value(&supplied) != owner_value {
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
        };
        let (state, detail) = process_state(Some(&owner));
        return Err(format!(
            "owner process generation mismatch for slot {slot}; recorded owner is {state}: {detail}"
        ));
    }
    assert_caller(&supplied)?;
    if record
        .get("handoff")
        .map(|value| !value.is_null())
        .unwrap_or(false)
    {
        return Err(format!(
            "slot {slot} already has a recorded finish; its handoff is immutable"
        ));
    }
    crate::hold_write::refuse_target_storage(root, slot)?;
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let path = sidecar_path(&cfg.control, &selected, slot, generation);
    let mut existing_schema1 = false;
    let existing: Option<Value> = if path.exists() || path.is_symlink() {
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
        if keys != expected {
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
    let events = if let EventLogStatus::Derived = event_log_status(&directory) {
        load_event_chain(&directory, &selected)?
    } else {
        Vec::new()
    };
    let published = publication(&events, slot, generation);
    if let Some((sequence, intended, completed)) = &published {
        let intent_event = events
            .iter()
            .find(|event| event.get("sequence").and_then(Value::as_i64) == Some(*sequence));
        if let Some(event) = intent_event {
            let payload = event.get("payload").cloned().unwrap_or(Value::Null);
            validate_intent(&payload, slot, generation)?;
            if payload.get("writer") != Some(&identity_value(&supplied)) {
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
    let digest = format!("{:x}", Sha256::digest(&contents));
    let mut artifact = artifact_obj(
        &selected,
        slot,
        generation,
        &utc_now(),
        &source,
        &source_identity,
        &contents,
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
