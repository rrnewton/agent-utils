//! Native Rust `reset` for one headless turn-runner worker.
//!
//! Ports the runtime half of the Python operation: under the runtime home's
//! registry lock, refuse unknown/TUI/busy workers, rotate the harness
//! session identity, append the reset marker to the transcript, and save the
//! registry. The registry row is kept as ordered JSON and only its
//! `session_id` changes; every other field is rewritten exactly as loaded,
//! with the same defaults the existing reader applies to older rows.
//! `migrate` and `repair` deliberately stay with the other edition until the
//! herdr-subagents port; see the campaign editions model.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde_json::{json, Map, Value};

type Result<T> = std::result::Result<T, String>;

const ROW_FIELDS: [&str; 18] = [
    "name",
    "harness",
    "backend",
    "tmux_target",
    "cwd",
    "model",
    "session_id",
    "status",
    "runner_pid",
    "runner_started_at",
    "next_seq",
    "created_at",
    "last_turn_at",
    "mode",
    "presentation_pane",
    "codex_bypass_permissions",
    "harness_args",
    "runner_boot_id",
];

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// One normalized registry row, in the field order the registry writes.
fn normalize_row(raw: &Map<String, Value>) -> Map<String, Value> {
    let text = |key: &str| raw.get(key).and_then(Value::as_str).map(str::to_owned);
    let target = text("tmux_target").unwrap_or_default();
    let herdr_tab = {
        let bytes = target.as_bytes();
        bytes.len() >= 4 && bytes[0] == b'w' && bytes[1].is_ascii_digit() && target.contains(":t")
    };
    let backend = text("backend").unwrap_or_else(|| {
        if herdr_tab {
            "herdr".to_owned()
        } else {
            "tmux".to_owned()
        }
    });
    let mut row = Map::new();
    for field in ROW_FIELDS {
        let value = match field {
            "backend" => Value::String(backend.clone()),
            "mode" => Value::String(text("mode").unwrap_or_else(|| "headless".to_owned())),
            "presentation_pane" => raw.get("presentation_pane").cloned().unwrap_or(Value::Null),
            "runner_started_at" => raw.get("runner_started_at").cloned().unwrap_or(Value::Null),
            "codex_bypass_permissions" => raw
                .get("codex_bypass_permissions")
                .cloned()
                .unwrap_or(Value::Bool(false)),
            "harness_args" => raw
                .get("harness_args")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "runner_boot_id" => raw.get("runner_boot_id").cloned().unwrap_or(Value::Null),
            _ => raw.get(field).cloned().unwrap_or(Value::Null),
        };
        row.insert(field.to_owned(), value);
    }
    row
}

fn random_uuid4() -> String {
    let mut bytes = [0_u8; 16];
    let filled = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
        .is_ok();
    if !filled {
        // Fall back to time and pid mixing; uniqueness matters, secrecy does not.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seed = now ^ (std::process::id() as u128) << 64;
        bytes.copy_from_slice(&seed.to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn pending_count(home: &Path, name: &str) -> usize {
    let inbox = home.join("state").join(name).join("inbox");
    std::fs::read_dir(inbox)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .count()
        })
        .unwrap_or(0)
}

/// Reset one worker in the runtime home at `home`; returns the worker
/// response document (`result` plus the post-reset `record`).
pub fn reset_agent(home: &Path, name: &str) -> Result<Value> {
    if !valid_name(name) {
        return Err(format!(
            "invalid agent name {name:?}: use letters, digits, '-' or '_' (<=64 chars, must start alphanumeric)"
        ));
    }
    std::fs::create_dir_all(home)
        .map_err(|error| format!("cannot create runtime home {}: {error}", home.display()))?;
    let lock_path = home.join(".registry.lock");
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| format!("cannot open runtime registry lock: {error}"))?;
    FileExt::lock_exclusive(&lock_file)
        .map_err(|error| format!("cannot lock runtime registry: {error}"))?;
    let registry_path = home.join("registry.json");
    let mut rows: Vec<Map<String, Value>> = Vec::new();
    if registry_path.exists() {
        let text = std::fs::read_to_string(&registry_path)
            .map_err(|error| format!("cannot read runtime registry: {error}"))?;
        let parsed: Value = serde_json::from_str(&text).map_err(|error| {
            format!(
                "registry.json is corrupt ({error}); inspect {} by hand",
                registry_path.display()
            )
        })?;
        let array = parsed
            .as_array()
            .ok_or_else(|| "registry.json must be a JSON array".to_owned())?;
        for row in array {
            let object = row
                .as_object()
                .ok_or_else(|| "registry.json rows must be JSON objects".to_owned())?;
            rows.push(normalize_row(object));
        }
    }
    let index = rows
        .iter()
        .position(|row| row.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| format!("unknown agent {name:?}"))?;
    let mode = rows[index]
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("headless")
        .to_owned();
    if mode == "tui" {
        return Err(
            "interactive TUI context is owned by its visible Codex process; use /new or /resume in that tab"
                .to_owned(),
        );
    }
    let status = rows[index]
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if status == "busy" || pending_count(home, name) > 0 {
        return Err(format!(
            "agent {name:?} has an active or queued turn; reset it at an idle task boundary"
        ));
    }
    let previous_session_id = rows[index]
        .get("session_id")
        .cloned()
        .unwrap_or(Value::Null);
    let harness = rows[index]
        .get("harness")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let new_session = if harness == "muse" {
        Value::String(random_uuid4())
    } else {
        Value::Null
    };
    rows[index].insert("session_id".to_owned(), new_session);
    let transcript: PathBuf = home.join("state").join(name).join("transcript.log");
    if let Some(parent) = transcript.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create transcript directory: {error}"))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .map_err(|error| format!("cannot append to transcript: {error}"))?;
    file.write_all(b"=== CONTEXT RESET ===\n")
        .map_err(|error| format!("cannot append to transcript: {error}"))?;
    // Save under the lock: rows sorted by name, two-space JSON, trailing LF.
    let mut sorted = rows.clone();
    sorted.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(b.get("name").and_then(Value::as_str).unwrap_or_default())
    });
    let payload = Value::Array(
        sorted
            .into_iter()
            .map(|mut row| {
                row.remove("runner_boot_id");
                Value::Object(row)
            })
            .collect::<Vec<_>>(),
    );
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("cannot encode runtime registry: {error}"))?;
    let temporary = home.join("registry.json.tmp");
    std::fs::write(&temporary, format!("{text}\n"))
        .map_err(|error| format!("cannot write runtime registry: {error}"))?;
    std::fs::rename(&temporary, &registry_path)
        .map_err(|error| format!("cannot replace runtime registry: {error}"))?;
    let record = rows.into_iter().nth(index).map(Value::Object);
    Ok(json!({
        "result": {
            "name": name,
            "previous_session_id": previous_session_id,
            "transcript": transcript.to_string_lossy(),
        },
        "record": record,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_rotates_only_the_session() {
        let home = std::env::temp_dir().join(format!("worker-reset-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("state/worker/inbox")).expect("home");
        std::fs::write(
            home.join("registry.json"),
            serde_json::to_string(&json!([{
                "name": "worker", "harness": "codex", "backend": "tmux",
                "tmux_target": "s:1", "cwd": "/work", "model": null,
                "session_id": "old-session", "status": "idle", "runner_pid": 12,
                "runner_started_at": null, "next_seq": 1,
                "created_at": "2026-01-01T00:00:00Z", "last_turn_at": null
            }]))
            .expect("json"),
        )
        .expect("registry");
        let response = reset_agent(&home, "worker").expect("reset");
        assert_eq!(
            response["result"]["previous_session_id"],
            json!("old-session")
        );
        assert_eq!(response["record"]["session_id"], Value::Null);
        let transcript =
            std::fs::read_to_string(home.join("state/worker/transcript.log")).expect("transcript");
        assert_eq!(transcript, "=== CONTEXT RESET ===\n");
        assert!(reset_agent(&home, "missing").is_err_and(|error| error.contains("unknown agent")));
        let _ = std::fs::remove_dir_all(&home);
    }
}
