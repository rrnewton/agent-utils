//! Per-agent filesystem layout and the active-harness bridge.
//!
//! State lives below `HERDR_SUBAGENTS_HOME` (the foreign runtime base),
//! independently of the installed package. This slice covers the layout
//! paths, name validation, durable JSON writes, and reading/recording the
//! `active-harness.json` ownership record through
//! [`super::ownership::parse_harness_record`] — parity with the matching
//! `py/agentctl/foreign/lib.py` functions. The registry-wide record index
//! (`AgentRecord`, registry lock/read) lands in a later slice.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::ownership::{parse_harness_record, ForeignOwnershipError, HarnessRecord};

/// The foreign runtime base: `$HERDR_SUBAGENTS_HOME`, else
/// `$XDG_STATE_HOME/herdr-agent/foreign`, else `~/.local/state/herdr-agent/foreign`.
pub fn foreign_base() -> PathBuf {
    foreign_base_from(
        std::env::var_os("HERDR_SUBAGENTS_HOME").as_deref(),
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn foreign_base_from(
    foreign_home: Option<&std::ffi::OsStr>,
    xdg_state_home: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> PathBuf {
    if let Some(base) = foreign_home.filter(|value| !value.is_empty()) {
        return expand_home(PathBuf::from(base), home);
    }
    let state = match xdg_state_home.filter(|value| !value.is_empty()) {
        Some(state) => expand_home(PathBuf::from(state), home),
        None => home.map_or_else(
            || PathBuf::from(".local/state"),
            |home| PathBuf::from(home).join(".local/state"),
        ),
    };
    state.join("herdr-agent/foreign")
}

fn expand_home(path: PathBuf, home: Option<&std::ffi::OsStr>) -> PathBuf {
    let Some(home) = home else { return path };
    match path.strip_prefix("~") {
        Ok(rest) => PathBuf::from(home).join(rest),
        Err(_) => path,
    }
}

/// Validate an agent name for structured API callers
/// (`^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$`, as in Python's `require_valid_name`).
pub fn require_valid_name(name: &str) -> Result<&str, ForeignOwnershipError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if valid {
        Ok(name)
    } else {
        Err(ForeignOwnershipError {
            code: "invalid_name",
            message: format!(
                "invalid agent name '{name}': use letters, digits, '-' or '_' \
                 (<=64 chars, must start alphanumeric)"
            ),
        })
    }
}

/// The state directory (`BASE/state`) holding every agent directory.
pub fn state_dir(base: &Path) -> PathBuf {
    base.join("state")
}

/// The runtime directory for one validated worker name (`STATE/<name>`).
pub fn agent_dir_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(state_dir(base).join(require_valid_name(name)?))
}

/// The ordered pending-message directory for one worker.
pub fn inbox_dir_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("inbox"))
}

/// The directory retaining messages consumed by the worker.
pub fn processed_dir_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("processed"))
}

/// Requests claimed before headless execution and awaiting completion.
pub fn inflight_dir_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("inflight"))
}

/// Retained malformed or possibly executed requests requiring inspection.
pub fn failed_dir_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("failed"))
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

fn random_hex6() -> String {
    let mut bytes = [0_u8; 3];
    if let Ok(mut urandom) = std::fs::File::open("/dev/urandom") {
        use std::io::Read as _;
        let _ = urandom.read_exact(&mut bytes);
    } else {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
            ^ u128::from(std::process::id());
        bytes = [(seed >> 16) as u8, (seed >> 8) as u8, seed as u8];
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Write one JSON document durably: exclusive temp file in the same
/// directory, fsync, rename over the target, then fsync the directory
/// (Python `_write_durable_json`; semantic parity — the bytes are compact
/// `serde_json`, not Python's spaced `json.dump` separators).
pub fn write_durable_json(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let name = path.file_name().map_or_else(
        || "document".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let temporary = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        random_hex6()
    ));
    let result = (|| {
        let mut stream = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        serde_json::to_writer(&mut stream, value)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        stream.sync_all()?;
        drop(stream);
        std::fs::rename(&temporary, path)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

/// The durable ownership record of a worker's running harness tree.
pub fn active_harness_path_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("active-harness.json"))
}

/// Durably record one turn's harness tree before the harness may exec.
/// `recorded_at` is the caller's registry timestamp (Python `now_iso()`;
/// supplied by the runner slice that owns timestamp generation).
pub fn record_active_harness_at(
    base: &Path,
    name: &str,
    record: &HarnessRecord,
    recorded_at: &str,
) -> Result<(), ForeignOwnershipError> {
    if record.leader.started_at.is_none()
        || record.runner.started_at.is_none()
        || record.leader.boot_id.is_none()
        || record.generation.is_none()
    {
        return Err(ForeignOwnershipError {
            code: "harness_identity_unknown",
            message: "cannot establish active harness process identity".to_owned(),
        });
    }
    let path = active_harness_path_at(base, name)?;
    write_durable_json(&path, &record.document(recorded_at)).map_err(|error| {
        ForeignOwnershipError {
            code: "harness_identity_unknown",
            message: format!("cannot write active harness identity: {error}"),
        }
    })
}

/// Return the recorded harness ownership, `None` when none is recorded.
pub fn read_active_harness_at(
    base: &Path,
    name: &str,
) -> Result<Option<HarnessRecord>, ForeignOwnershipError> {
    let path = active_harness_path_at(base, name)?;
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ForeignOwnershipError {
                code: "harness_identity_unknown",
                message: format!("cannot read active harness identity: {error}"),
            });
        }
    };
    let value: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|error| ForeignOwnershipError {
            code: "harness_identity_unknown",
            message: format!("cannot read active harness identity: {error}"),
        })?;
    parse_harness_record(&value).map(Some)
}

/// Remove the ownership record only while it still names this generation.
/// An unreadable record is kept, as in Python.
pub fn remove_active_harness_at(
    base: &Path,
    name: &str,
    record: &HarnessRecord,
) -> Result<(), ForeignOwnershipError> {
    let path = active_harness_path_at(base, name)?;
    let current = match read_active_harness_at(base, name) {
        Ok(current) => current,
        Err(_) => return Ok(()),
    };
    if current.as_ref() != Some(record) {
        return Ok(());
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ForeignOwnershipError {
                code: "harness_identity_unknown",
                message: format!("cannot remove active harness identity: {error}"),
            });
        }
    }
    if let Some(parent) = path.parent() {
        sync_directory(parent).map_err(|error| ForeignOwnershipError {
            code: "harness_identity_unknown",
            message: format!("cannot remove active harness identity: {error}"),
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BOOT: &str = "123e4567-e89b-42d3-a456-426614174000";
    const GEN: &str = "0123456789abcdef0123456789abcdef";

    fn fake_base(test: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("agentctl-b3-4-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("state/worker-1")).unwrap();
        root
    }

    fn v1_record() -> HarnessRecord {
        parse_harness_record(&json!({
            "schema": "agentctl-headless-harness/v1",
            "runner_pid": 100,
            "runner_started_at": "111",
            "runner_boot_id": BOOT,
            "pid": 200,
            "started_at": "222",
            "boot_id": BOOT,
            "process_group": 200,
            "generation": GEN,
            "containment": "process",
            "recorded_at": "2026-11-07T00:00:00Z",
        }))
        .unwrap()
    }

    #[test]
    fn names_follow_the_python_shape() {
        for valid in ["a", "A-1_x", "worker-1", &"x".repeat(64)] {
            assert_eq!(require_valid_name(valid).unwrap(), valid);
        }
        for invalid in ["", "-x", "_x", "a b", "a/b", "a.b", "..", &"x".repeat(65)] {
            let err = require_valid_name(invalid).unwrap_err();
            assert_eq!(err.code, "invalid_name", "name: {invalid:?}");
            assert!(agent_dir_at(Path::new("/base"), invalid).is_err());
        }
    }

    #[test]
    fn layout_paths_match_python() {
        let base = Path::new("/base");
        assert_eq!(state_dir(base), PathBuf::from("/base/state"));
        assert_eq!(
            agent_dir_at(base, "worker-1").unwrap(),
            PathBuf::from("/base/state/worker-1")
        );
        assert_eq!(
            inbox_dir_at(base, "worker-1").unwrap(),
            PathBuf::from("/base/state/worker-1/inbox")
        );
        assert_eq!(
            processed_dir_at(base, "worker-1").unwrap(),
            PathBuf::from("/base/state/worker-1/processed")
        );
        assert_eq!(
            inflight_dir_at(base, "worker-1").unwrap(),
            PathBuf::from("/base/state/worker-1/inflight")
        );
        assert_eq!(
            failed_dir_at(base, "worker-1").unwrap(),
            PathBuf::from("/base/state/worker-1/failed")
        );
        assert_eq!(
            active_harness_path_at(base, "worker-1").unwrap(),
            PathBuf::from("/base/state/worker-1/active-harness.json")
        );
    }

    #[test]
    fn base_resolution_prefers_the_foreign_home() {
        use std::ffi::OsStr;
        assert_eq!(
            foreign_base_from(
                Some(OsStr::new("/foreign")),
                Some(OsStr::new("/xdg")),
                Some(OsStr::new("/home")),
            ),
            PathBuf::from("/foreign")
        );
        assert_eq!(
            foreign_base_from(None, Some(OsStr::new("/xdg")), Some(OsStr::new("/home"))),
            PathBuf::from("/xdg/herdr-agent/foreign")
        );
        assert_eq!(
            foreign_base_from(None, None, Some(OsStr::new("/home"))),
            PathBuf::from("/home/.local/state/herdr-agent/foreign")
        );
        assert_eq!(
            foreign_base_from(
                Some(OsStr::new("~/foreign")),
                None,
                Some(OsStr::new("/home"))
            ),
            PathBuf::from("/home/foreign")
        );
    }

    #[test]
    fn active_harness_round_trips_through_disk() {
        let base = fake_base("roundtrip");
        let record = v1_record();
        assert_eq!(read_active_harness_at(&base, "worker-1").unwrap(), None);

        record_active_harness_at(&base, "worker-1", &record, "2026-11-07T00:00:00Z").unwrap();
        let path = active_harness_path_at(&base, "worker-1").unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["generation"], json!(GEN));
        // The durable write leaves no temp file behind.
        let leftovers: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp leftovers: {leftovers:?}");

        assert_eq!(
            read_active_harness_at(&base, "worker-1").unwrap(),
            Some(record.clone())
        );

        // A different generation's record is not removed.
        let other = parse_harness_record(&json!({
            "pid": 300, "started_at": "333", "runner_pid": 100
        }))
        .unwrap();
        remove_active_harness_at(&base, "worker-1", &other).unwrap();
        assert!(path.exists());

        remove_active_harness_at(&base, "worker-1", &record).unwrap();
        assert!(!path.exists());
        assert_eq!(read_active_harness_at(&base, "worker-1").unwrap(), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn active_harness_refusals_carry_the_python_codes() {
        let base = fake_base("refusals");
        // An incomplete identity cannot be recorded.
        let legacy = parse_harness_record(&json!({
            "pid": 200, "started_at": "222", "runner_pid": 100
        }))
        .unwrap();
        let err = record_active_harness_at(&base, "worker-1", &legacy, "2026-11-07T00:00:00Z")
            .unwrap_err();
        assert_eq!(err.code, "harness_identity_unknown");
        assert_eq!(
            err.message,
            "cannot establish active harness process identity"
        );

        // Malformed JSON on disk is unreadable evidence, not absence.
        let path = active_harness_path_at(&base, "worker-1").unwrap();
        std::fs::write(&path, "{not json").unwrap();
        let err = read_active_harness_at(&base, "worker-1").unwrap_err();
        assert_eq!(err.code, "harness_identity_unknown");
        assert!(err.message.contains("cannot read"), "{}", err.message);

        // Valid JSON naming no identity is a malformed record.
        std::fs::write(&path, "{\"pid\": 1}\n").unwrap();
        assert!(read_active_harness_at(&base, "worker-1").is_err());

        // An unreadable record is kept on remove, as in Python.
        remove_active_harness_at(&base, "worker-1", &v1_record()).unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn durable_json_overwrites_atomically() {
        let base = fake_base("durable");
        let path = base.join("doc.json");
        write_durable_json(&path, &json!({"a": 1})).unwrap();
        write_durable_json(&path, &json!({"a": 2})).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.ends_with(b"\n"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            json!({"a": 2})
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
