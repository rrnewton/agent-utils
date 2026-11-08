//! Per-agent filesystem layout and the active-harness bridge.
//!
//! State lives below `HERDR_SUBAGENTS_HOME` (the foreign runtime base),
//! independently of the installed package. This slice covers the layout
//! paths, name validation, durable JSON writes, reading/recording the
//! `active-harness.json` ownership record through
//! [`super::ownership::parse_harness_record`], and the registry-wide
//! [`AgentRecord`] index with its flock-guarded read/repair/write paths —
//! parity with the matching `py/agentctl/foreign/lib.py` functions.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use super::ownership::{
    kill_harness_tree_at, parse_harness_record, read_stat_at, valid_boot_id, ForeignOwnershipError,
    HarnessRecord, ProcessIdentity, RUNNER_SCHEMA,
};

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
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    write_durable_bytes(path, &bytes)
}

fn write_durable_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
        stream.write_all(bytes)?;
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

/// Kill this worker's recorded harness tree through exact handles only.
/// Returns true when a process was signalled. A tree still present after
/// the bounded wait keeps its record and errors with
/// `harness_cleanup_incomplete`; a legacy record is resolved without
/// signalling anything it cannot prove.
pub fn terminate_active_harness(record: &AgentRecord) -> Result<bool, ForeignOwnershipError> {
    terminate_active_harness_at(&foreign_base(), Path::new("/proc"), record)
}

/// [`terminate_active_harness`] against explicit foreign-base and procfs roots.
pub fn terminate_active_harness_at(
    base: &Path,
    proc_root: &Path,
    record: &AgentRecord,
) -> Result<bool, ForeignOwnershipError> {
    let Some(harness) = read_active_harness_at(base, &record.name)? else {
        return Ok(false);
    };
    if !owned_by_record(record, &harness) {
        return Ok(false);
    }
    if harness.legacy() {
        return resolve_legacy_harness_at(base, proc_root, &record.name, &harness);
    }
    let report = kill_harness_tree_at(proc_root, &harness, None, false, false, None)?;
    if !report.complete() {
        let path = active_harness_path_at(base, &record.name)?;
        return Err(registry_error(
            "harness_cleanup_incomplete",
            format!(
                "harness pid {} of {}: {}; its ownership record {} is kept for a later attempt",
                harness.leader.pid,
                python_repr(&record.name),
                report.describe(),
                path.display()
            ),
        ));
    }
    remove_active_harness_at(base, &record.name, &harness)?;
    Ok(report.killed > 0)
}

fn resolve_legacy_harness_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
    harness: &HarnessRecord,
) -> Result<bool, ForeignOwnershipError> {
    let path = active_harness_path_at(base, name)?;
    let leader = read_stat_at(proc_root, harness.leader.pid)?;
    let stale = match leader {
        None => true,
        Some(leader) => {
            harness.leader.started_at.as_deref() != Some(leader.starttime.to_string().as_str())
                || ["Z", "X", "x"].contains(&leader.state.as_str())
        }
    };
    if stale {
        remove_missing_ok(&path)?;
        return Ok(false);
    }
    Err(registry_error(
        "harness_identity_legacy",
        format!(
            "harness pid {} of {} was recorded by an older runner without a boot identity or generation, so it is not signalled; stop its runner, whose own watchdog owns that group, or inspect pid {} and stop it by hand, then remove {}",
            harness.leader.pid,
            python_repr(name),
            harness.leader.pid,
            path.display()
        ),
    ))
}

/// The headless turn-runner mode.
pub const HEADLESS_MODE: &str = "headless";
/// The interactive TUI mode.
pub const TUI_MODE: &str = "tui";
/// Worker modes the foreign runtime can record.
pub const SUPPORTED_MODES: [&str; 2] = [HEADLESS_MODE, TUI_MODE];
/// Presentation backends the foreign runtime can record.
pub const SUPPORTED_BACKENDS: [&str; 2] = ["tmux", "herdr"];

/// Return a supported presentation backend or a structured operation error.
pub fn require_supported_backend(backend: &str) -> Result<&str, ForeignOwnershipError> {
    if SUPPORTED_BACKENDS.contains(&backend) {
        Ok(backend)
    } else {
        Err(registry_error(
            "unsupported_backend",
            format!(
                "unsupported backend {}; supported backends: {}",
                python_repr(backend),
                SUPPORTED_BACKENDS.join(", ")
            ),
        ))
    }
}

/// Return a supported worker mode or a structured operation error.
pub fn require_supported_mode(mode: &str) -> Result<&str, ForeignOwnershipError> {
    if SUPPORTED_MODES.contains(&mode) {
        Ok(mode)
    } else {
        Err(registry_error(
            "unsupported_mode",
            format!(
                "unsupported agent mode {}; supported modes: {}",
                python_repr(mode),
                SUPPORTED_MODES.join(", ")
            ),
        ))
    }
}

fn registry_error(code: &'static str, message: String) -> ForeignOwnershipError {
    ForeignOwnershipError { code, message }
}

fn registry_invalid(message: String) -> ForeignOwnershipError {
    registry_error("registry_invalid", message)
}

fn registry_io(context: &str, path: &Path, error: &std::io::Error) -> ForeignOwnershipError {
    registry_error(
        "registry_io",
        format!("cannot {context} {}: {error}", path.display()),
    )
}

/// Python's single-quoted `repr` for the strings these errors name.
fn python_repr(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        match ch {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\x{:02x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('\'');
    out
}

/// A process ID paired with its kernel start time and boot identity.
pub type RunnerIdentity = ProcessIdentity;

/// A durable queued turn with sequence, prompt, model, reasoning effort,
/// and enqueue time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Turn sequence number.
    pub seq: i64,
    /// Prompt text.
    pub text: String,
    /// Per-turn model override.
    pub model: Option<String>,
    /// Enqueue timestamp.
    pub queued_at: String,
    /// Per-turn reasoning effort override.
    pub effort: Option<String>,
}

impl Message {
    /// Load a queued turn, accepting records that omit optional model and
    /// effort overrides (Python `Message.from_dict` shape).
    pub fn from_dict(row: &Map<String, Value>) -> Result<Self, ForeignOwnershipError> {
        let required = ["seq", "text", "queued_at"]
            .into_iter()
            .filter(|field| !row.contains_key(*field))
            .collect::<Vec<_>>();
        if !required.is_empty() {
            return Err(registry_invalid(format!(
                "queued turn missing keys: [{}]",
                required
                    .iter()
                    .map(|field| python_repr(field))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(Self {
            seq: required_int(row, "seq")?,
            text: required_string(row, "text")?,
            model: optional_string(row, "model")?,
            queued_at: required_string(row, "queued_at")?,
            effort: optional_string(row, "effort")?,
        })
    }

    /// Read a queued turn from its message file.
    pub fn from_path(path: &Path) -> Result<Self, ForeignOwnershipError> {
        let bytes =
            std::fs::read(path).map_err(|error| registry_io("read queued turn", path, &error))?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            registry_invalid(format!(
                "queued turn {} is not valid JSON: {error}",
                path.display()
            ))
        })?;
        let Some(row) = value.as_object() else {
            return Err(registry_invalid(format!(
                "queued turn {} must contain a JSON object",
                path.display()
            )));
        };
        Self::from_dict(row)
    }

    /// Serialize as written to the inbox (all keys, optionals as null).
    pub fn to_dict(&self) -> Value {
        serde_json::json!({
            "seq": self.seq,
            "text": self.text,
            "model": self.model,
            "queued_at": self.queued_at,
            "effort": self.effort,
        })
    }
}

/// One row of the registry (one live agent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentRecord {
    /// Worker name.
    pub name: String,
    /// Harness identifier (`codex`, `agy`, or `muse` for created rows).
    pub harness: String,
    /// Presentation backend.
    pub backend: String,
    /// Tmux target, or Herdr tab id for the Herdr backend (compat name).
    pub tmux_target: String,
    /// Working directory recorded at creation.
    pub cwd: String,
    /// Model override, when recorded.
    pub model: Option<String>,
    /// Harness session id, when one exists.
    pub session_id: Option<String>,
    /// `starting | idle | busy | error | quota_exhausted | auth_error`.
    pub status: String,
    /// Runner process number, once started.
    pub runner_pid: Option<i64>,
    /// Runner kernel start ticks, once recorded.
    pub runner_started_at: Option<String>,
    /// Next message sequence number.
    pub next_seq: i64,
    /// Creation timestamp.
    pub created_at: String,
    /// Last turn timestamp, when a turn has run.
    pub last_turn_at: Option<String>,
    /// Worker mode; missing on V1/V2 rows, which were always headless.
    pub mode: String,
    /// Herdr pane owning an interactive TUI; headless rows omit it in spirit.
    pub presentation_pane: Option<String>,
    /// Whether Codex may bypass its native permissions for this worker.
    pub codex_bypass_permissions: bool,
    /// Literal non-secret harness options recorded at creation.
    pub harness_args: Vec<String>,
    /// Boot identity of the runner, from its sidecar; never part of the row.
    #[serde(skip)]
    pub runner_boot_id: Option<String>,
}

impl AgentRecord {
    /// Serialize the registry row; the boot identity lives in its sidecar.
    pub fn to_dict(&self) -> Value {
        serde_json::to_value(self).expect("AgentRecord serializes to JSON")
    }

    /// The recorded runner as a boot-bound identity, or `None` before it starts.
    pub fn runner_identity(&self) -> Option<RunnerIdentity> {
        self.runner_pid.map(|pid| {
            ProcessIdentity::new(
                pid,
                self.runner_started_at.clone(),
                self.runner_boot_id.clone(),
            )
        })
    }

    /// Load a worker record under the default foreign base, restoring omitted
    /// presentation fields from preserved identity metadata.
    pub fn from_dict(row: &Map<String, Value>) -> Result<Self, ForeignOwnershipError> {
        Self::from_dict_at(&foreign_base(), row)
    }

    /// [`AgentRecord::from_dict`] against an explicit foreign base.
    pub fn from_dict_at(
        base: &Path,
        row: &Map<String, Value>,
    ) -> Result<Self, ForeignOwnershipError> {
        const ALL_FIELDS: [&str; 18] = [
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
        const OMISSIBLE: [&str; 7] = [
            "runner_started_at",
            "backend",
            "mode",
            "presentation_pane",
            "codex_bypass_permissions",
            "harness_args",
            "runner_boot_id",
        ];
        let mut missing: Vec<&str> = ALL_FIELDS
            .into_iter()
            .filter(|field| !row.contains_key(*field) && !OMISSIBLE.contains(field))
            .collect();
        if !missing.is_empty() {
            missing.sort_unstable();
            let shown_name = match row.get("name") {
                Some(Value::String(name)) => python_repr(name),
                Some(other) => scalar_string(other).unwrap_or_else(|| other.to_string()),
                None => "None".to_owned(),
            };
            let shown_missing = missing
                .iter()
                .map(|field| python_repr(field))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(registry_invalid(format!(
                "registry row for {shown_name} missing keys: [{shown_missing}]"
            )));
        }

        let preserved = load_presentation_identity_at(base, row)?;
        let tmux_target = required_string(row, "tmux_target")?;
        let backend = truthy_scalar_string(row.get("backend"))
            .or_else(|| preserved.as_ref().map(|saved| saved.backend.clone()))
            .unwrap_or_else(|| {
                if herdr_tab_id(&tmux_target) {
                    "herdr".to_owned()
                } else {
                    "tmux".to_owned()
                }
            });
        let mode = require_supported_mode(
            &truthy_scalar_string(row.get("mode"))
                .or_else(|| preserved.as_ref().map(|saved| saved.mode.clone()))
                .unwrap_or_else(|| HEADLESS_MODE.to_owned()),
        )?
        .to_owned();
        let presentation_pane = match row.get("presentation_pane") {
            Some(value) if !value.is_null() => Some(scalar_string(value).ok_or_else(|| {
                registry_invalid(
                    "registry row field 'presentation_pane' must be a string".to_owned(),
                )
            })?),
            _ => preserved
                .as_ref()
                .filter(|_| mode == TUI_MODE)
                .map(|saved| saved.presentation_pane.clone()),
        };
        let codex_bypass_permissions = load_codex_permission_policy_at(base, row)?;
        let harness_args = match row.get("harness_args") {
            None => Vec::new(),
            Some(Value::Array(items))
                if items.iter().all(|item| {
                    item.as_str()
                        .is_some_and(|item| !item.is_empty() && !item.contains('\0'))
                }) =>
            {
                items
                    .iter()
                    .map(|item| item.as_str().expect("validated string").to_owned())
                    .collect()
            }
            Some(_) => {
                return Err(registry_invalid(
                    "registry row has invalid harness_args".to_owned(),
                ));
            }
        };
        let runner_pid = match row.get("runner_pid") {
            Some(Value::Null) => None,
            Some(value) => Some(parse_record_int(value).ok_or_else(|| {
                registry_invalid("registry row field 'runner_pid' must be an integer".to_owned())
            })?),
            None => None,
        };
        let runner_started_at = optional_string(row, "runner_started_at")?;
        let name = required_string(row, "name")?;
        let created_at = required_string(row, "created_at")?;
        let runner_boot_id = load_runner_boot_id_at(
            base,
            &name,
            &created_at,
            runner_pid,
            runner_started_at.as_deref(),
        );
        Ok(Self {
            name,
            harness: required_string(row, "harness")?,
            backend,
            tmux_target,
            cwd: required_string(row, "cwd")?,
            model: optional_string(row, "model")?,
            session_id: optional_string(row, "session_id")?,
            status: required_string(row, "status")?,
            runner_pid,
            runner_started_at,
            next_seq: required_int(row, "next_seq")?,
            created_at,
            last_turn_at: optional_string(row, "last_turn_at")?,
            mode,
            presentation_pane,
            codex_bypass_permissions,
            harness_args,
            runner_boot_id,
        })
    }
}

/// Registry rows by worker name, in the sorted order saves write.
pub type AgentRegistry = BTreeMap<String, AgentRecord>;

impl Default for AgentRecord {
    fn default() -> Self {
        Self {
            name: String::new(),
            harness: String::new(),
            backend: String::new(),
            tmux_target: String::new(),
            cwd: String::new(),
            model: None,
            session_id: None,
            status: String::new(),
            runner_pid: None,
            runner_started_at: None,
            next_seq: 0,
            created_at: String::new(),
            last_turn_at: None,
            mode: HEADLESS_MODE.to_owned(),
            presentation_pane: None,
            codex_bypass_permissions: false,
            harness_args: Vec::new(),
            runner_boot_id: None,
        }
    }
}

fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(true) => Some("True".to_owned()),
        Value::Bool(false) => Some("False".to_owned()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn required_string(row: &Map<String, Value>, key: &str) -> Result<String, ForeignOwnershipError> {
    row.get(key)
        .and_then(scalar_string)
        .ok_or_else(|| registry_invalid(format!("registry row field '{key}' must be a string")))
}

fn optional_string(
    row: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ForeignOwnershipError> {
    match row.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => scalar_string(value).map(Some).ok_or_else(|| {
            registry_invalid(format!("registry row field '{key}' must be a string"))
        }),
    }
}

/// Python truthiness for the scalar fallback reads (`raw or default`).
fn truthy_scalar_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(Value::Bool(true)) => Some("True".to_owned()),
        Some(Value::Number(number)) if number.as_f64().is_some_and(|number| number != 0.0) => {
            Some(number.to_string())
        }
        _ => None,
    }
}

fn parse_record_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok())),
        Value::String(text) => parse_python_int(text),
        _ => None,
    }
}

fn parse_python_int(text: &str) -> Option<i64> {
    let text = text.trim_matches(|ch: char| ch.is_ascii_whitespace());
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1_i64, rest),
        None => (1_i64, text.strip_prefix('+').unwrap_or(text)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    digits
        .replace('_', "")
        .parse::<i64>()
        .ok()
        .and_then(|value| value.checked_mul(sign))
}

fn required_int(row: &Map<String, Value>, key: &str) -> Result<i64, ForeignOwnershipError> {
    row.get(key)
        .and_then(parse_record_int)
        .ok_or_else(|| registry_invalid(format!("registry row field '{key}' must be an integer")))
}

fn herdr_tab_id(target: &str) -> bool {
    // `^w[^:]+:t[0-9]+$`
    let Some(rest) = target.strip_prefix('w') else {
        return false;
    };
    let Some((middle, tab)) = rest.split_once(':') else {
        return false;
    };
    !middle.is_empty()
        && !middle.contains(':')
        && tab.len() > 1
        && tab.starts_with('t')
        && tab[1..].bytes().all(|byte| byte.is_ascii_digit())
}

/// TUI presentation data that survives old registry writers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationIdentity {
    /// Presentation backend.
    pub backend: String,
    /// Worker mode (always the TUI mode for a valid sidecar).
    pub mode: String,
    /// Herdr pane owning the TUI.
    pub presentation_pane: String,
}

/// The durable sidecar path used to recover a worker presentation identity.
pub fn presentation_identity_path_at(
    base: &Path,
    name: &str,
) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("presentation.json"))
}

/// The sidecar binding the registry row's runner identity to a boot.
pub fn runner_identity_path_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("runner-identity.json"))
}

/// The sidecar preserving a worker's Codex permission policy.
pub fn permission_policy_path_at(
    base: &Path,
    name: &str,
) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("permissions.json"))
}

fn load_runner_boot_id_at(
    base: &Path,
    name: &str,
    created_at: &str,
    runner_pid: Option<i64>,
    runner_started_at: Option<&str>,
) -> Option<String> {
    let runner_pid = runner_pid?;
    let path = runner_identity_path_at(base, name).ok()?;
    let raw: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let raw = raw.as_object()?;
    let matches = raw.get("schema").and_then(Value::as_str) == Some(RUNNER_SCHEMA)
        && raw.get("name").and_then(Value::as_str) == Some(name)
        && raw.get("created_at").and_then(Value::as_str) == Some(created_at)
        && raw.get("runner_pid").and_then(parse_record_int) == Some(runner_pid)
        && match raw.get("runner_started_at") {
            None | Some(Value::Null) => runner_started_at.is_none(),
            Some(Value::String(text)) => runner_started_at == Some(text.as_str()),
            _ => false,
        };
    if !matches {
        return None;
    }
    raw.get("runner_boot_id")
        .and_then(Value::as_str)
        .filter(|boot_id| valid_boot_id(boot_id))
        .map(str::to_owned)
}

fn load_presentation_identity_at(
    base: &Path,
    row: &Map<String, Value>,
) -> Result<Option<PresentationIdentity>, ForeignOwnershipError> {
    let nonempty = |key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    };
    let (Some(name), Some(target), Some(created_at)) = (
        nonempty("name"),
        nonempty("tmux_target"),
        nonempty("created_at"),
    ) else {
        return Ok(None);
    };
    let Ok(path) = presentation_identity_path_at(base, name) else {
        return Ok(None);
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(registry_io("read presentation identity", &path, &error)),
    };
    let raw: Value = serde_json::from_slice(&bytes).map_err(|error| {
        registry_invalid(format!(
            "presentation identity {} is corrupt ({error}); inspect it before sending work",
            path.display()
        ))
    })?;
    let Some(raw) = raw.as_object() else {
        return Err(registry_invalid(format!(
            "presentation identity {} must contain a JSON object",
            path.display()
        )));
    };
    if raw.get("name").and_then(Value::as_str) != Some(name)
        || raw.get("tmux_target").and_then(Value::as_str) != Some(target)
        || raw.get("created_at").and_then(Value::as_str) != Some(created_at)
    {
        return Ok(None);
    }
    let (Some(backend), Some(mode), Some(pane)) = (
        raw.get("backend").and_then(Value::as_str),
        raw.get("mode").and_then(Value::as_str),
        raw.get("presentation_pane")
            .and_then(Value::as_str)
            .filter(|pane| !pane.is_empty()),
    ) else {
        return Err(registry_invalid(format!(
            "presentation identity {} is missing a valid backend, mode, or pane id",
            path.display()
        )));
    };
    require_supported_backend(backend)?;
    if require_supported_mode(mode)? != TUI_MODE {
        return Err(registry_invalid(format!(
            "presentation identity {} must describe a TUI agent",
            path.display()
        )));
    }
    Ok(Some(PresentationIdentity {
        backend: backend.to_owned(),
        mode: mode.to_owned(),
        presentation_pane: pane.to_owned(),
    }))
}

fn load_codex_permission_policy_at(
    base: &Path,
    row: &Map<String, Value>,
) -> Result<bool, ForeignOwnershipError> {
    let invalid = |message: String| registry_error("invalid_permission_policy", message);
    let value = if let Some(value) = row.get("codex_bypass_permissions") {
        Some(value.clone())
    } else {
        // Old runners rewrite rows without this field. The sidecar preserves
        // a new worker's explicit choice; only genuinely old records retain
        // the historical default (true).
        let mut value = Value::Bool(true);
        if let Some(name) = row.get("name").and_then(scalar_string) {
            if let Ok(path) = permission_policy_path_at(base, &name) {
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let saved: Value = serde_json::from_slice(&bytes).map_err(|error| {
                            invalid(format!(
                                "cannot read worker permissions {}: {error}",
                                path.display()
                            ))
                        })?;
                        let Some(saved) = saved.as_object() else {
                            return Err(invalid(format!(
                                "worker permissions {} must contain an object",
                                path.display()
                            )));
                        };
                        if saved.get("name") == row.get("name")
                            && saved.get("created_at") == row.get("created_at")
                        {
                            value = saved
                                .get("codex_bypass_permissions")
                                .cloned()
                                .unwrap_or(Value::Null);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(invalid(format!(
                            "cannot read worker permissions {}: {error}",
                            path.display()
                        )));
                    }
                }
            }
        }
        Some(value)
    };
    match value {
        Some(Value::Bool(value)) => Ok(value),
        _ => Err(invalid(
            "recorded codex_bypass_permissions must be a boolean".to_owned(),
        )),
    }
}

fn remove_missing_ok(path: &Path) -> Result<(), ForeignOwnershipError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(registry_io("remove", path, &error)),
    }
}

fn save_presentation_identity_at(
    base: &Path,
    record: &AgentRecord,
) -> Result<(), ForeignOwnershipError> {
    let path = presentation_identity_path_at(base, &record.name)?;
    let Some(pane) = record
        .presentation_pane
        .as_deref()
        .filter(|_| record.mode == TUI_MODE)
    else {
        return remove_missing_ok(&path);
    };
    let dir = agent_dir_at(base, &record.name)?;
    std::fs::create_dir_all(&dir).map_err(|error| registry_io("create", &dir, &error))?;
    let payload = serde_json::json!({
        "name": record.name,
        "created_at": record.created_at,
        "backend": record.backend,
        "tmux_target": record.tmux_target,
        "mode": record.mode,
        "presentation_pane": pane,
    });
    write_durable_json(&path, &payload).map_err(|error| registry_io("write", &path, &error))
}

fn save_codex_permission_policy_at(
    base: &Path,
    record: &AgentRecord,
) -> Result<(), ForeignOwnershipError> {
    let path = permission_policy_path_at(base, &record.name)?;
    let dir = agent_dir_at(base, &record.name)?;
    std::fs::create_dir_all(&dir).map_err(|error| registry_io("create", &dir, &error))?;
    let payload = serde_json::json!({
        "name": record.name,
        "created_at": record.created_at,
        "codex_bypass_permissions": record.codex_bypass_permissions,
    });
    write_durable_json(&path, &payload).map_err(|error| registry_io("write", &path, &error))
}

fn save_runner_identity_at(base: &Path, record: &AgentRecord) -> Result<(), ForeignOwnershipError> {
    let path = runner_identity_path_at(base, &record.name)?;
    let (Some(runner_pid), Some(runner_boot_id)) = (record.runner_pid, &record.runner_boot_id)
    else {
        return remove_missing_ok(&path);
    };
    let dir = agent_dir_at(base, &record.name)?;
    std::fs::create_dir_all(&dir).map_err(|error| registry_io("create", &dir, &error))?;
    let payload = serde_json::json!({
        "schema": RUNNER_SCHEMA,
        "name": record.name,
        "created_at": record.created_at,
        "runner_pid": runner_pid,
        "runner_started_at": record.runner_started_at,
        "runner_boot_id": runner_boot_id,
    });
    write_durable_json(&path, &payload).map_err(|error| registry_io("write", &path, &error))
}

/// Whether a harness record belongs to this registry row's runner.
pub fn owned_by_record(record: &AgentRecord, harness: &HarnessRecord) -> bool {
    Some(harness.runner.pid) == record.runner_pid
        && (record.runner_started_at.is_none()
            || harness.runner.started_at == record.runner_started_at)
}

/// The registry file (`BASE/registry.json`).
pub fn registry_path(base: &Path) -> PathBuf {
    base.join("registry.json")
}

/// The registry lock file (`BASE/.registry.lock`).
pub fn registry_lock_path(base: &Path) -> PathBuf {
    base.join(".registry.lock")
}

struct FileLock {
    file: std::fs::File,
}

impl FileLock {
    fn acquire(path: &Path, exclusive: bool) -> Result<Self, ForeignOwnershipError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o644)
            .open(path)
            .map_err(|error| registry_io("open registry lock", path, &error))?;
        let operation = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        };
        loop {
            // SAFETY: flock takes the descriptor of the open lock file.
            if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
                return Ok(Self { file });
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(registry_io("lock registry", path, &error));
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // SAFETY: the descriptor still names this guard's lock file.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn load_unlocked_at(base: &Path) -> Result<(AgentRegistry, bool), ForeignOwnershipError> {
    let path = registry_path(base);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((AgentRegistry::new(), false));
        }
        Err(error) => return Err(registry_io("read registry", &path, &error)),
    };
    let raw: Value = serde_json::from_slice(&bytes).map_err(|error| {
        registry_invalid(format!(
            "registry.json is corrupt ({error}); inspect {} by hand",
            path.display()
        ))
    })?;
    let Some(rows) = raw.as_array() else {
        return Err(registry_invalid(format!(
            "registry.json must be a JSON array, found {}",
            python_type_name(&raw)
        )));
    };
    let mut agents = AgentRegistry::new();
    let mut presentation_repair_needed = false;
    for row in rows {
        let Some(row) = row.as_object() else {
            return Err(registry_invalid(
                "registry.json rows must be JSON objects".to_owned(),
            ));
        };
        let record = AgentRecord::from_dict_at(base, row)?;
        if record.mode == TUI_MODE {
            let current = record.to_dict();
            if ["backend", "mode", "presentation_pane"]
                .iter()
                .any(|field| row.get(*field) != current.get(*field))
            {
                presentation_repair_needed = true;
            }
        }
        agents.insert(record.name.clone(), record);
    }
    Ok((agents, presentation_repair_needed))
}

fn save_unlocked_at(base: &Path, agents: &AgentRegistry) -> Result<(), ForeignOwnershipError> {
    // Write TUI identities before the registry, as in Python: a crash between
    // the writes cannot publish a TUI row without its fallback.
    for record in agents.values() {
        save_presentation_identity_at(base, record)?;
        save_codex_permission_policy_at(base, record)?;
        save_runner_identity_at(base, record)?;
    }
    let rows: Vec<&AgentRecord> = agents.values().collect();
    let mut text = serde_json::to_string_pretty(&rows)
        .map_err(|error| registry_invalid(format!("cannot serialize registry: {error}")))?;
    text.push('\n');
    let path = registry_path(base);
    write_durable_bytes(&path, text.as_bytes())
        .map_err(|error| registry_io("write registry", &path, &error))
}

fn ensure_base(base: &Path) -> Result<(), ForeignOwnershipError> {
    std::fs::create_dir_all(base).map_err(|error| registry_io("create", base, &error))
}

/// Read the registry under the default foreign base, repairing stripped
/// live-TUI presentation fields.
pub fn read_registry() -> Result<AgentRegistry, ForeignOwnershipError> {
    read_registry_at(&foreign_base())
}

/// [`read_registry`] against an explicit foreign base.
pub fn read_registry_at(base: &Path) -> Result<AgentRegistry, ForeignOwnershipError> {
    ensure_base(base)?;
    let lock_path = registry_lock_path(base);
    let (agents, presentation_repair_needed) = {
        let _lock = FileLock::acquire(&lock_path, false)?;
        load_unlocked_at(base)?
    };
    if !presentation_repair_needed {
        return Ok(agents);
    }
    // Only take the exclusive lock when a sidecar proved a legacy writer
    // dropped a live TUI's fields; re-load so a concurrent repair wins.
    let _lock = FileLock::acquire(&lock_path, true)?;
    let (agents, presentation_repair_needed) = load_unlocked_at(base)?;
    if presentation_repair_needed {
        save_unlocked_at(base, &agents)?;
    }
    Ok(agents)
}

/// Exclusive read-modify-write guard over the registry. Call
/// [`RegistryLock::save`] to write the (possibly mutated) mapping back;
/// dropping the guard without saving discards changes, matching an
/// exceptional exit from Python's `registry_lock` context manager.
pub struct RegistryLock {
    base: PathBuf,
    agents: AgentRegistry,
    _lock: FileLock,
}

impl RegistryLock {
    /// The loaded registry rows.
    pub fn agents(&self) -> &AgentRegistry {
        &self.agents
    }

    /// Mutable registry rows; persist them with [`RegistryLock::save`].
    pub fn agents_mut(&mut self) -> &mut AgentRegistry {
        &mut self.agents
    }

    /// Write the mapping back (sidecars first) and release the lock.
    pub fn save(self) -> Result<(), ForeignOwnershipError> {
        save_unlocked_at(&self.base, &self.agents)
    }
}

/// Take the registry lock under the default foreign base.
pub fn registry_lock() -> Result<RegistryLock, ForeignOwnershipError> {
    registry_lock_at(&foreign_base())
}

/// [`registry_lock`] against an explicit foreign base.
pub fn registry_lock_at(base: &Path) -> Result<RegistryLock, ForeignOwnershipError> {
    ensure_base(base)?;
    let lock = FileLock::acquire(&registry_lock_path(base), true)?;
    let (agents, _presentation_repair_needed) = load_unlocked_at(base)?;
    Ok(RegistryLock {
        base: base.to_path_buf(),
        agents,
        _lock: lock,
    })
}

/// Run `operation` under the exclusive registry lock, saving only when it
/// succeeds (Python `with registry_lock() as agents:`).
pub fn with_registry_lock_at<T>(
    base: &Path,
    operation: impl FnOnce(&mut AgentRegistry) -> Result<T, ForeignOwnershipError>,
) -> Result<T, ForeignOwnershipError> {
    let mut lock = registry_lock_at(base)?;
    let value = operation(lock.agents_mut())?;
    lock.save()?;
    Ok(value)
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

    fn record_row(name: &str) -> Map<String, Value> {
        json!({
            "name": name,
            "harness": "codex",
            "tmux_target": "subagents:worker",
            "cwd": "/tmp/work",
            "model": null,
            "session_id": "session-1",
            "status": "idle",
            "runner_pid": 4321,
            "runner_started_at": "999",
            "next_seq": 3,
            "created_at": "2026-11-07T00:00:00-05:00",
            "last_turn_at": null,
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn sample_record(name: &str) -> AgentRecord {
        AgentRecord {
            name: name.to_owned(),
            harness: "codex".to_owned(),
            backend: "tmux".to_owned(),
            tmux_target: "subagents:worker".to_owned(),
            cwd: "/tmp/work".to_owned(),
            session_id: Some("session-1".to_owned()),
            status: "idle".to_owned(),
            runner_pid: Some(4321),
            runner_started_at: Some("999".to_owned()),
            next_seq: 3,
            created_at: "2026-11-07T00:00:00-05:00".to_owned(),
            runner_boot_id: Some(BOOT.to_owned()),
            ..AgentRecord::default()
        }
    }

    #[test]
    fn from_dict_restores_legacy_defaults_and_infers_backend() {
        let base = fake_base("from-dict");
        let mut row = record_row("worker-1");
        row.insert("runner_pid".to_owned(), json!("4321"));
        row.insert("next_seq".to_owned(), json!("7"));
        let record = AgentRecord::from_dict_at(&base, &row).unwrap();
        assert_eq!(record.backend, "tmux");
        assert_eq!(record.mode, HEADLESS_MODE);
        assert_eq!(record.presentation_pane, None);
        assert!(
            record.codex_bypass_permissions,
            "genuinely old rows keep the historical default"
        );
        assert!(record.harness_args.is_empty());
        assert_eq!(record.runner_pid, Some(4321));
        assert_eq!(record.next_seq, 7);
        assert_eq!(record.runner_boot_id, None);
        assert_eq!(
            record.runner_identity(),
            Some(ProcessIdentity::new(4321, Some("999".to_owned()), None))
        );
        let written = record.to_dict();
        assert_eq!(written["backend"], json!("tmux"));
        assert_eq!(written["mode"], json!("headless"));
        assert!(written.get("runner_boot_id").is_none());

        row.insert("tmux_target".to_owned(), json!("w12:t34"));
        assert_eq!(
            AgentRecord::from_dict_at(&base, &row).unwrap().backend,
            "herdr"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn from_dict_rejects_missing_and_invalid_rows() {
        let base = fake_base("invalid-rows");
        let mut row = record_row("worker-1");
        row.remove("cwd");
        let err = AgentRecord::from_dict_at(&base, &row).unwrap_err();
        assert_eq!(err.code, "registry_invalid");
        assert!(
            err.message.contains("missing keys: ['cwd']"),
            "{}",
            err.message
        );

        let mut row = record_row("worker-1");
        row.insert("mode".to_owned(), json!("weird"));
        assert_eq!(
            AgentRecord::from_dict_at(&base, &row).unwrap_err().code,
            "unsupported_mode"
        );

        for bad_args in [json!([""]), json!([1]), json!(["a\0b"]), json!("--flag")] {
            let mut row = record_row("worker-1");
            row.insert("harness_args".to_owned(), bad_args);
            let err = AgentRecord::from_dict_at(&base, &row).unwrap_err();
            assert!(
                err.message.contains("invalid harness_args"),
                "{}",
                err.message
            );
        }
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

    #[test]
    fn permission_sidecar_preserves_an_explicit_choice() {
        let base = fake_base("permissions");
        let row = record_row("worker-1");
        assert!(
            AgentRecord::from_dict_at(&base, &row)
                .unwrap()
                .codex_bypass_permissions
        );
        let path = permission_policy_path_at(&base, "worker-1").unwrap();
        write_durable_json(
            &path,
            &json!({
                "name": "worker-1",
                "created_at": "2026-11-07T00:00:00-05:00",
                "codex_bypass_permissions": false,
            }),
        )
        .unwrap();
        assert!(
            !AgentRecord::from_dict_at(&base, &row)
                .unwrap()
                .codex_bypass_permissions
        );
        write_durable_json(
            &path,
            &json!({
                "name": "worker-1",
                "created_at": "another-generation",
                "codex_bypass_permissions": false,
            }),
        )
        .unwrap();
        assert!(
            AgentRecord::from_dict_at(&base, &row)
                .unwrap()
                .codex_bypass_permissions
        );
        let mut explicit = record_row("worker-1");
        explicit.insert("codex_bypass_permissions".to_owned(), json!("yes"));
        assert_eq!(
            AgentRecord::from_dict_at(&base, &explicit)
                .unwrap_err()
                .code,
            "invalid_permission_policy"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn registry_round_trips_sorted_with_sidecars() {
        let base = fake_base("roundtrip-registry");
        let mut second = sample_record("worker-2");
        second.runner_boot_id = None;
        with_registry_lock_at(&base, |agents| {
            agents.insert("worker-2".to_owned(), second);
            agents.insert("worker-1".to_owned(), sample_record("worker-1"));
            Ok(())
        })
        .unwrap();
        let written: Value =
            serde_json::from_slice(&std::fs::read(registry_path(&base)).unwrap()).unwrap();
        let names: Vec<&str> = written
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["worker-1", "worker-2"]);
        assert!(written[0].get("runner_boot_id").is_none());
        let loaded = read_registry_at(&base).unwrap();
        assert_eq!(loaded["worker-1"].runner_boot_id.as_deref(), Some(BOOT));
        assert_eq!(loaded["worker-2"].runner_boot_id, None);
        assert!(permission_policy_path_at(&base, "worker-1")
            .unwrap()
            .exists());
        let runner_sidecar: Value = serde_json::from_slice(
            &std::fs::read(runner_identity_path_at(&base, "worker-1").unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(runner_sidecar["schema"], json!(RUNNER_SCHEMA));
        std::fs::write(
            runner_identity_path_at(&base, "worker-1").unwrap(),
            "{corrupt",
        )
        .unwrap();
        assert_eq!(
            read_registry_at(&base).unwrap()["worker-1"].runner_boot_id,
            None
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn read_registry_repairs_stripped_tui_fields() {
        let base = fake_base("repair-tui");
        let mut tui = sample_record("worker-1");
        tui.backend = "herdr".to_owned();
        tui.tmux_target = "w12:t34".to_owned();
        tui.mode = TUI_MODE.to_owned();
        tui.presentation_pane = Some("w12:p56".to_owned());
        tui.runner_pid = None;
        tui.runner_started_at = None;
        tui.runner_boot_id = None;
        with_registry_lock_at(&base, |agents| {
            agents.insert("worker-1".to_owned(), tui);
            Ok(())
        })
        .unwrap();
        // Simulate a legacy writer dropping TUI-only fields.
        let mut stripped = record_row("worker-1");
        stripped.insert("tmux_target".to_owned(), json!("w12:t34"));
        stripped.insert("runner_pid".to_owned(), Value::Null);
        std::fs::write(registry_path(&base), json!([stripped]).to_string()).unwrap();
        let loaded = read_registry_at(&base).unwrap();
        let record = &loaded["worker-1"];
        assert_eq!(record.backend, "herdr");
        assert_eq!(record.mode, TUI_MODE);
        assert_eq!(record.presentation_pane.as_deref(), Some("w12:p56"));
        let repaired: Value =
            serde_json::from_slice(&std::fs::read(registry_path(&base)).unwrap()).unwrap();
        assert_eq!(repaired[0]["mode"], json!("tui"));
        assert_eq!(repaired[0]["presentation_pane"], json!("w12:p56"));
        assert!(!owned_by_record(record, &v1_record()));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn registry_lock_discards_a_failed_operation() {
        let base = fake_base("lock-discard");
        with_registry_lock_at(&base, |agents| {
            agents.insert("worker-1".to_owned(), sample_record("worker-1"));
            Ok(())
        })
        .unwrap();
        let result: Result<(), _> = with_registry_lock_at(&base, |agents| {
            agents.clear();
            Err(registry_invalid("simulated failure".to_owned()))
        });
        assert!(result.is_err());
        assert_eq!(read_registry_at(&base).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn terminate_active_harness_kills_and_removes_the_record() {
        use super::super::ownership::{current_boot_id, read_stat, Containment};
        use std::os::unix::process::ExitStatusExt as _;
        if !super::super::ownership::pidfds_supported() {
            return;
        }
        let base = fake_base("terminate");
        let mut leader = std::process::Command::new("setsid")
            .args(["sleep", "10"])
            .spawn()
            .expect("spawn session leader");
        let leader_pid = i64::from(leader.id());
        // `setsid` may not have created the new session yet; the recorded
        // session must be the leader's own, read only once it leads it.
        let mut leader_stat = read_stat(leader_pid).unwrap().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while leader_stat.session != leader_pid && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
            leader_stat = read_stat(leader_pid).unwrap().unwrap();
        }
        assert_eq!(leader_stat.session, leader_pid);
        // The recorded runner must be its own process: using this test
        // process would make every parallel test's children look like
        // child-of-the-runner members to the census.
        let mut runner_proc = std::process::Command::new("sleep")
            .arg("10")
            .spawn()
            .expect("spawn runner stand-in");
        let runner_pid = i64::from(runner_proc.id());
        let own_stat = read_stat(runner_pid).unwrap().unwrap();
        let boot = current_boot_id().unwrap();
        let harness = HarnessRecord {
            runner: ProcessIdentity::new(
                runner_pid,
                Some(own_stat.starttime.to_string()),
                Some(boot.clone()),
            ),
            leader: ProcessIdentity::new(
                leader_pid,
                Some(leader_stat.starttime.to_string()),
                Some(boot),
            ),
            process_group: Some(leader_stat.session),
            generation: Some(GEN.to_owned()),
            containment: Containment::Process,
            cgroup: None,
        };
        record_active_harness_at(&base, "worker-1", &harness, "2026-11-07T00:00:00Z").unwrap();
        let mut record = sample_record("worker-1");
        record.runner_pid = Some(runner_pid);
        record.runner_started_at = Some(own_stat.starttime.to_string());
        assert!(terminate_active_harness_at(&base, Path::new("/proc"), &record).unwrap());
        assert!(!active_harness_path_at(&base, "worker-1").unwrap().exists());
        assert_eq!(leader.wait().unwrap().signal(), Some(libc::SIGKILL));
        assert!(
            runner_proc.try_wait().unwrap().is_none(),
            "runner is never a member"
        );
        runner_proc.kill().unwrap();
        runner_proc.wait().unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn terminate_active_harness_never_signals_a_legacy_record() {
        use super::super::ownership::read_stat;
        let base = fake_base("terminate-legacy");
        let mut child = std::process::Command::new("sleep")
            .arg("10")
            .spawn()
            .expect("spawn legacy leader");
        let pid = i64::from(child.id());
        let stat = read_stat(pid).unwrap().unwrap();
        let path = active_harness_path_at(&base, "worker-1").unwrap();
        std::fs::write(
            &path,
            json!({"pid": pid, "started_at": stat.starttime.to_string(), "runner_pid": 100})
                .to_string(),
        )
        .unwrap();
        let mut record = sample_record("worker-1");
        record.runner_pid = Some(100);
        record.runner_started_at = None;
        let err = terminate_active_harness_at(&base, Path::new("/proc"), &record).unwrap_err();
        assert_eq!(err.code, "harness_identity_legacy");
        assert!(path.exists());
        assert!(
            child.try_wait().unwrap().is_none(),
            "child must be untouched"
        );

        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!terminate_active_harness_at(&base, Path::new("/proc"), &record).unwrap());
        assert!(!path.exists(), "stale legacy record is removed");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn message_round_trips_with_omitted_optionals() {
        let base = fake_base("message");
        let path = base.join("000000000007.json");
        std::fs::write(
            &path,
            "{\"seq\": 7, \"text\": \"hello\", \"queued_at\": \"2026-11-07T00:00:00-05:00\"}\n",
        )
        .unwrap();
        let msg = Message::from_path(&path).unwrap();
        assert_eq!(msg.seq, 7);
        assert_eq!(msg.model, None);
        assert_eq!(msg.effort, None);
        assert_eq!(msg.to_dict()["text"], json!("hello"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn corrupt_registry_shapes_are_reported() {
        let root =
            std::env::temp_dir().join(format!("agentctl-b3-4-{}-corrupt", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(read_registry_at(&root).unwrap().is_empty());
        std::fs::write(registry_path(&root), "{}").unwrap();
        let err = read_registry_at(&root).unwrap_err();
        assert!(
            err.message.contains("must be a JSON array, found dict"),
            "{}",
            err.message
        );
        std::fs::write(registry_path(&root), "{broken").unwrap();
        assert!(read_registry_at(&root)
            .unwrap_err()
            .message
            .contains("is corrupt"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
