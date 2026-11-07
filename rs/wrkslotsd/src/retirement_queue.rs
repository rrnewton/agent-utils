//! Read-only `retirement-queue`: the generation-bound queue derived
//! from append-only handoff-read events (the Python
//! `_current_retirement_candidates` plus `_emit_retirement_queue`).
//! This command never changes a slot or sidecar.

use std::path::Path;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{load_cfg, slot_directory, Cfg};
use crate::config_validate::load_config;
use crate::ensure_states::refuse_partial_state_allow_seals;
use crate::global_state::load_event_chain;
use crate::handoff_state::{load_sidecar_artifact, sidecar_path};
use crate::heartbeat_write::release_in_effect;
use crate::hold_state::parse_hold;
use crate::hold_write::{active_document, hold_from_events};
use crate::remove_write::liveness_answer;

type Result<T> = std::result::Result<T, String>;

pub(crate) struct HandoffRead {
    pub(crate) sha256: String,
    pub(crate) enqueued: bool,
    pub(crate) sequence: i64,
    recorded_at: String,
    source: Option<String>,
    source_path: Option<String>,
    source_identity: Option<Value>,
}

impl HandoffRead {
    pub(crate) fn provenance_complete(&self) -> bool {
        self.source.is_some()
    }
}

pub(crate) fn latest_handoff_read(
    events: &[Value],
    slot: &str,
    generation: i64,
) -> Result<Option<HandoffRead>> {
    let mut latest: Option<HandoffRead> = None;
    for event in events {
        if event.get("kind").and_then(Value::as_str) != Some("handoff-read") {
            continue;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if payload.get("slot").and_then(Value::as_str) != Some(slot)
            || payload.get("generation").and_then(Value::as_i64) != Some(generation)
        {
            continue;
        }
        let digest = payload
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if digest.len() != 64 || !digest.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err("handoff-read event sha256 is not a SHA-256 digest".to_string());
        }
        let enqueued = match payload.get("retirement_enqueued") {
            None => false,
            Some(Value::Bool(flag)) => *flag,
            _ => return Err("handoff-read event retirement_enqueued must be boolean".to_string()),
        };
        let values = [
            payload.get("source"),
            payload.get("source_path"),
            payload.get("source_identity"),
        ];
        let (source, source_path, source_identity) = if values.iter().all(|v| v.is_none()) {
            (None, None, None)
        } else if values.iter().any(|v| v.is_none()) {
            return Err("handoff-read event has incomplete source provenance".to_string());
        } else {
            let source = values[0]
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !matches!(source.as_str(), "write-handoff" | "legacy-slot-file") {
                return Err("handoff-read event source is invalid".to_string());
            }
            (
                Some(source),
                Some(
                    values[1]
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ),
                Some(values[2].cloned().unwrap_or(Value::Null)),
            )
        };
        latest = Some(HandoffRead {
            sha256: digest,
            enqueued,
            sequence: event.get("sequence").and_then(Value::as_i64).unwrap_or(0),
            recorded_at: event
                .get("recorded_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            source,
            source_path,
            source_identity,
        });
    }
    Ok(latest)
}

#[derive(Clone)]
struct Artifact {
    path: String,
    source: String,
    sha256: String,
    source_path: Option<String>,
    source_identity: Option<Value>,
    provenance_complete: bool,
    contents: String,
}

fn publication(events: &[Value], slot: &str, generation: i64) -> Option<(i64, bool)> {
    let mut intents: Vec<i64> = Vec::new();
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
                    intents.push(sequence);
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
    let latest = intents.into_iter().max()?;
    Some((latest, completed.contains(&latest)))
}

fn resolve_artifact(
    cfg: &Cfg,
    selected: &str,
    record: &Value,
    events: &[Value],
) -> std::result::Result<Option<Artifact>, String> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let slot_path = slot_directory(cfg, slot, slot_type)?;
    let sidecar = match load_sidecar_artifact(cfg, selected, slot, slot_type, generation) {
        Ok(found) => found.map(|artifact| {
            let value = artifact.value;
            Artifact {
                path: sidecar_path(&cfg.control, selected, slot, generation)
                    .to_string_lossy()
                    .into_owned(),
                source: value
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                sha256: value
                    .get("sha256")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                source_path: value
                    .get("source_path")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                source_identity: value.get("source_identity").cloned(),
                provenance_complete: true,
                contents: value
                    .get("contents_utf8")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }
        }),
        Err(error) => return Err(error),
    };
    let published = publication(events, slot, generation);
    if sidecar.is_none() {
        if let Some((_sequence, done)) = published {
            let (state, remedy) = if done {
                ("completed", "restore the exact completed sidecar")
            } else {
                (
                    "outstanding",
                    "have the exact owner retry write-handoff or recover its authenticated temp",
                )
            };
            return Err(format!(
                "slot {slot} has a {state} owner-authenticated handoff write intent but no canonical sidecar; preserve the slot and {remedy}"
            ));
        }
    }
    let legacy_path = slot_path.join("HANDOFF.md");
    let legacy = if legacy_path.exists() || legacy_path.is_symlink() {
        let contents = std::fs::read(&legacy_path).unwrap_or_default();
        let text = String::from_utf8_lossy(&contents).into_owned();
        let digest = format!("{:x}", Sha256::digest(&contents));
        let metadata = std::fs::metadata(&legacy_path).ok();
        #[cfg(unix)]
        let (device, inode, size) = {
            use std::os::unix::fs::MetadataExt;
            metadata
                .as_ref()
                .map(|meta| (meta.dev(), meta.ino(), meta.size()))
                .unwrap_or((0, 0, contents.len() as u64))
        };
        #[cfg(not(unix))]
        let (device, inode, size) = (0u64, 0u64, contents.len() as u64);
        let _ = (device, inode);
        Some(Artifact {
            path: legacy_path.to_string_lossy().into_owned(),
            source: "legacy-slot-file".to_string(),
            sha256: digest.clone(),
            source_path: Some(legacy_path.to_string_lossy().into_owned()),
            source_identity: Some(json!({
                "identity": [device, inode],
                "size": size,
                "sha256": digest,
            })),
            provenance_complete: true,
            contents: text,
        })
    } else {
        None
    };
    if let (Some(sidecar), Some(legacy)) = (&sidecar, &legacy) {
        let (sidecar, legacy) = (sidecar.clone(), legacy);
        if sidecar.contents != legacy.contents || sidecar.sha256 != legacy.sha256 {
            return Err(format!(
                "slot {slot} contains an unread HANDOFF.md: its handoff sidecar disagrees with the legacy file; preserve both files and reconcile their exact contents"
            ));
        }
        if sidecar.source == "legacy-slot-file"
            && (sidecar.source_path != legacy.source_path
                || sidecar.source_identity != legacy.source_identity)
        {
            return Err(format!(
                "slot {slot} contains an unread HANDOFF.md: its legacy file identity changed even though its bytes are unchanged; preserve both files for manual recovery; published sidecars are immutable"
            ));
        }
        return Ok(Some(sidecar));
    }
    Ok(sidecar.or(legacy))
}

fn release_obj(cfg: &Cfg, record: &Value, events: &[Value]) -> Result<Option<Map<String, Value>>> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut latest: Option<Value> = None;
    for event in events {
        if event.get("kind").and_then(Value::as_str) != Some("owner-released") {
            continue;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if payload.get("slot").and_then(Value::as_str) == Some(slot)
            && payload.get("generation").and_then(Value::as_i64) == Some(generation)
        {
            latest = Some(event.clone());
        }
    }
    let Some(event) = latest else {
        return Ok(None);
    };
    let payload = event.get("payload").cloned().unwrap_or(Value::Null);
    let recorded_at = event
        .get("recorded_at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let in_effect = release_in_effect(cfg, record, events)?.is_some();
    let void_reason: Option<String> = if in_effect {
        None
    } else if payload.get("agent").and_then(Value::as_str)
        != record.get("agent").and_then(Value::as_str)
    {
        Some(format!(
            "the slot now belongs to agent {}, not {}",
            record
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            payload
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ))
    } else if payload.get("heartbeat_at").and_then(Value::as_str)
        != record.get("heartbeat_at").and_then(Value::as_str)
    {
        Some(format!(
            "the heartbeat was renewed after the release (released at heartbeat {}, now {})",
            payload
                .get("heartbeat_at")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            record
                .get("heartbeat_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ))
    } else {
        Some("the handoff publication changed after the release".to_string())
    };
    let mut object = Map::new();
    object.insert(
        "state".to_string(),
        json!(if in_effect { "released" } else { "void" }),
    );
    object.insert(
        "summary".to_string(),
        json!(if in_effect {
            format!("RELEASED by live owner at {recorded_at}")
        } else {
            format!(
                "release at {recorded_at} no longer applies: {}",
                void_reason.clone().unwrap_or_default()
            )
        }),
    );
    object.insert(
        "event_sequence".to_string(),
        event.get("sequence").cloned().unwrap_or(Value::Null),
    );
    object.insert("recorded_at".to_string(), json!(recorded_at));
    object.insert(
        "heartbeat_at".to_string(),
        payload.get("heartbeat_at").cloned().unwrap_or(Value::Null),
    );
    object.insert(
        "owner".to_string(),
        payload.get("owner").cloned().unwrap_or(Value::Null),
    );
    object.insert(
        "handoff_sha256".to_string(),
        payload
            .get("handoff")
            .and_then(|handoff| handoff.get("sha256"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    object.insert(
        "void_reason".to_string(),
        void_reason.map(Value::String).unwrap_or(Value::Null),
    );
    Ok(Some(object))
}

/// Print the current retirement queue in the Python `human` or
/// `json` form.
pub(crate) fn retirement_queue(
    root: Option<&Path>,
    machine_override: Option<&str>,
    json_format: bool,
) -> Result<String> {
    let config = load_config(root, machine_override)?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    crate::config_validate::validate_name(&selected, "machine")?;
    refuse_partial_state_allow_seals(&cfg.control)?;
    let active = active_document(root, &cfg, &selected)?;
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    let events = if directory.is_dir() {
        load_event_chain(&directory, &selected)?
    } else {
        Vec::new()
    };
    let slots: Vec<Value> = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Last retirement-attempt sequence per (slot, generation, digest, read).
    let mut last_attempts: Vec<((String, i64, String, i64), i64)> = Vec::new();
    for event in &events {
        if event.get("kind").and_then(Value::as_str) != Some("retirement-attempted") {
            continue;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        let key = (
            payload
                .get("slot")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            payload
                .get("generation")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            payload
                .get("sha256")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            payload
                .get("handoff_read_sequence")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        );
        if let Some(sequence) = event.get("sequence").and_then(Value::as_i64) {
            last_attempts.push((key, sequence));
        }
    }
    struct Candidate {
        machine: String,
        slot: String,
        generation: i64,
        sha256: String,
        event_sequence: i64,
        enqueued_at: String,
        last_attempt_sequence: Option<i64>,
        artifact_path: Option<String>,
        artifact_source: Option<String>,
        blocked_reason: Option<String>,
        owner_release: Option<Map<String, Value>>,
        record: Value,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    for record in &slots {
        if record.get("slot_type").and_then(Value::as_str) != Some("agent") {
            continue;
        }
        let slot = record
            .get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let generation = record
            .get("generation")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let Some(latest) = latest_handoff_read(&events, &slot, generation)? else {
            continue;
        };
        if !latest.enqueued || !latest.provenance_complete() {
            continue;
        }
        let mut artifact_path: Option<String> = None;
        let mut artifact_source: Option<String> = None;
        let mut blocked_reason: Option<String> = None;
        match resolve_artifact(&cfg, &selected, record, &events) {
            Ok(None) => blocked_reason = Some("the exact handoff artifact is missing".to_string()),
            Ok(Some(artifact)) => {
                let matches = latest.provenance_complete()
                    && artifact.provenance_complete
                    && latest.sha256 == artifact.sha256
                    && latest.source == Some(artifact.source.clone())
                    && latest.source_path == artifact.source_path
                    && latest.source_identity == artifact.source_identity;
                if matches {
                    artifact_path = Some(artifact.path.clone());
                    artifact_source = Some(artifact.source.clone());
                } else {
                    blocked_reason = Some(
                        "the current handoff provenance differs from the enqueued read".to_string(),
                    );
                }
            }
            Err(error) => blocked_reason = Some(error),
        }
        // Holds: replayed from events when the log exists, else snapshots.
        let hold: Option<Value> = if !events.is_empty() {
            let held = hold_from_events(&events, &selected, &slot);
            if held.is_null() {
                None
            } else {
                Some(held)
            }
        } else {
            match parse_hold(root, Some(&selected), &slot) {
                Ok(value) if !value.is_null() => Some(value),
                Ok(_) => None,
                Err(_) => Some(json!({})),
            }
        };
        if let Some(hold) = &hold {
            let reason = hold.get("reason").and_then(Value::as_str);
            blocked_reason = Some(match reason {
                Some(text) if !text.trim().is_empty() => format!("slot is held: {text}"),
                _ => "slot is held: hold record is unreadable".to_string(),
            });
        }
        let owner_release = release_obj(&cfg, record, &events)?;
        let last_attempt = last_attempts
            .iter()
            .filter(|(key, _)| {
                key.0 == slot
                    && key.1 == generation
                    && key.2 == latest.sha256
                    && key.3 == latest.sequence
            })
            .map(|(_, sequence)| *sequence)
            .last();
        candidates.push(Candidate {
            machine: selected.clone(),
            slot,
            generation,
            sha256: latest.sha256.clone(),
            event_sequence: latest.sequence,
            enqueued_at: latest.recorded_at.clone(),
            last_attempt_sequence: last_attempt,
            artifact_path,
            artifact_source,
            blocked_reason,
            owner_release,
            record: record.clone(),
        });
    }
    // A live owner reported by the registered liveness authority is
    // never attempted; only an affirmative "alive" blocks.
    for candidate in candidates.iter_mut() {
        if candidate.blocked_reason.is_some() {
            continue;
        }
        let released = candidate
            .owner_release
            .as_ref()
            .map(|release| release.get("void_reason").is_some_and(|v| v.is_null()))
            .unwrap_or(false);
        if released {
            continue;
        }
        if let Ok(answer) = liveness_answer(&cfg, &config, &candidate.record) {
            if answer.state == "alive" {
                candidate.blocked_reason =
                    Some("registered liveness authority reports owner alive".to_string());
            }
        }
    }
    candidates.sort_by(|left, right| {
        let left_key = (
            left.last_attempt_sequence.is_some(),
            left.last_attempt_sequence.unwrap_or(left.event_sequence),
            left.event_sequence,
            left.slot.clone(),
        );
        let right_key = (
            right.last_attempt_sequence.is_some(),
            right.last_attempt_sequence.unwrap_or(right.event_sequence),
            right.event_sequence,
            right.slot.clone(),
        );
        left_key.cmp(&right_key)
    });
    if json_format {
        let pending: Vec<Value> = candidates
            .iter()
            .map(|candidate| {
                let mut object = Map::new();
                object.insert("machine".to_string(), json!(candidate.machine));
                object.insert("slot".to_string(), json!(candidate.slot));
                object.insert("generation".to_string(), json!(candidate.generation));
                object.insert("sha256".to_string(), json!(candidate.sha256));
                object.insert(
                    "event_sequence".to_string(),
                    json!(candidate.event_sequence),
                );
                object.insert("enqueued_at".to_string(), json!(candidate.enqueued_at));
                object.insert(
                    "last_attempt_sequence".to_string(),
                    candidate
                        .last_attempt_sequence
                        .map(|value| json!(value))
                        .unwrap_or(Value::Null),
                );
                object.insert(
                    "artifact_path".to_string(),
                    candidate
                        .artifact_path
                        .clone()
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
                object.insert(
                    "artifact_source".to_string(),
                    candidate
                        .artifact_source
                        .clone()
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
                object.insert(
                    "blocked_reason".to_string(),
                    candidate
                        .blocked_reason
                        .clone()
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
                if let Some(release) = &candidate.owner_release {
                    object.insert("owner_release".to_string(), Value::Object(release.clone()));
                }
                Value::Object(object)
            })
            .collect();
        let payload = json!({"schema": 1, "pending": pending});
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        ));
    }
    let mut out = format!("pending={}\n", candidates.len());
    for candidate in &candidates {
        let state = if candidate.blocked_reason.is_some() {
            "BLOCKED"
        } else {
            "PENDING"
        };
        let mut suffix = String::new();
        if let Some(reason) = &candidate.blocked_reason {
            suffix.push_str(&format!(" reason={reason}"));
        }
        if let Some(release) = &candidate.owner_release {
            if let Some(summary) = release.get("summary").and_then(Value::as_str) {
                suffix.push_str(&format!(" owner={summary}"));
            }
        }
        out.push_str(&format!(
            "{state}: {}/{} generation={} sha256={}{suffix}\n",
            candidate.machine, candidate.slot, candidate.generation, candidate.sha256
        ));
    }
    Ok(out)
}
