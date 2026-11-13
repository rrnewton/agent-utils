//! Step 1a of the ownerless-validation port (#1088): target
//! resolution, repository paths, directory identities, and the
//! terminal validation record's non-frozen binding, mirroring
//! the Python helpers of the same names. The frozen record
//! binding and checkout facts land in later steps; paths that
//! need them refuse explicitly.

use std::os::unix::fs::MetadataExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::active_state::Cfg;
use crate::config_validate::{ensure_no_symlink_components, relative_inside, validate_name};

type Result<T> = std::result::Result<T, String>;

/// Python `Path.as_posix` for the paths this module renders.
fn posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Lexical normpath (no filesystem access), as `os.path.normpath`.
fn normpath(raw: &str) -> String {
    let absolute = raw.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push(part);
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

/// The managed worktrees root (slots surface), mirroring
/// `_managed_worktrees_root` as the journal module computes it.
fn managed_worktrees_root(cfg: &Cfg) -> PathBuf {
    if cfg
        .worktrees
        .file_name()
        .is_some_and(|name| name == "slots")
    {
        cfg.worktrees
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| cfg.worktrees.clone())
    } else {
        cfg.worktrees.clone()
    }
}

/// Python `_validate_slots_directory`: `worktrees/validate` in
/// every layout (#1183).
pub(crate) fn validate_slots_directory(cfg: &Cfg) -> PathBuf {
    managed_worktrees_root(cfg).join("validate")
}

/// Python `_frozen_validation_directory`.
pub(crate) fn frozen_validation_directory(cfg: &Cfg) -> PathBuf {
    let name = cfg
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    cfg.root
        .parent()
        .map(|parent| parent.join(format!(".{name}-frozen-validate")))
        .unwrap_or_default()
}

/// (relative, target, validate slot identity when the
/// target is a validate slot).
pub(crate) type RecoveryTarget = (String, PathBuf, Option<(String, String)>);

/// Python `_validation_recovery_target`: (relative, target,
/// validate slot identity when the target is a validate slot).
pub(crate) fn validation_recovery_target(
    cfg: &Cfg,
    raw: &str,
    target_kind: &str,
) -> Result<RecoveryTarget> {
    if target_kind == "frozen-checkout" {
        let supplied = Path::new(raw);
        if !supplied.is_absolute() {
            return Err(
                "frozen validation checkout is not an absolute path; pass the exact absolute checkout path and retry"
                    .to_string(),
            );
        }
        if raw.split('/').any(|part| part == "..") {
            return Err(
                "frozen validation checkout path contains '..'; pass its normalized exact absolute path and retry"
                    .to_string(),
            );
        }
        let canonical = normpath(raw);
        if raw != canonical {
            return Err(format!(
                "frozen validation checkout must use its exact canonical absolute spelling; pass '{canonical}' and retry"
            ));
        }
        let target = PathBuf::from(&canonical);
        let frozen_root = frozen_validation_directory(cfg);
        let allowed = target.parent() == Some(frozen_root.as_path())
            && target
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("validate-fresh-"));
        if !allowed {
            return Err(format!(
                "frozen checkout path is outside the exact sibling validation directory: {}; preserve it or move it under {}/validate-fresh-* before retrying",
                target.display(),
                frozen_root.display()
            ));
        }
        let name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        validate_name(&name, "validation recovery path name")?;
        if let Some(parent) = cfg.root.parent() {
            ensure_no_symlink_components(parent, &target, "frozen validation recovery path")?;
        }
        return Ok((canonical, target, None));
    }
    let (relative, target) = relative_inside(&cfg.root, raw, "validation recovery path")?;
    let validate_root = validate_slots_directory(cfg);
    let allowed = match target_kind {
        "checkout" => {
            target.parent() == Some(validate_root.as_path())
                || (target.parent() == Some(cfg.root.join("ignored").as_path())
                    && target
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("validate-fresh-")))
        }
        "cargo-home" => {
            target
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("validate-cargo-"))
                && (target.parent() == Some(validate_root.as_path())
                    || target.parent()
                        == Some(cfg.root.join("ignored/validate/cargo-homes").as_path()))
        }
        other => return Err(format!("unknown validation recovery target kind '{other}'")),
    };
    if !allowed {
        return Err(format!(
            "{target_kind} path is outside the exact validation directories: {}",
            target.display()
        ));
    }
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    validate_name(&name, "validation recovery path name")?;
    let slot = if target.parent() == Some(validate_root.as_path()) {
        Some(("validate".to_string(), name))
    } else {
        None
    };
    Ok((relative, target, slot))
}

fn names_sibling_repository(parts: &[&str]) -> bool {
    parts.len() >= 2
        && parts[0] == ".."
        && parts[1..].iter().all(|part| !matches!(*part, "." | ".."))
}

/// Python `_resolved_repository_path`.
fn resolved_repository_path(
    cfg: &Cfg,
    stored: &str,
    unresolved: &Path,
    allowed_root: &Path,
    allow_managed: bool,
) -> Result<(String, PathBuf)> {
    ensure_no_symlink_components(allowed_root, unresolved, "repository")?;
    let absolute = unresolved.canonicalize().map_err(|_error| {
        format!(
            "source repository '{stored}' does not exist or cannot be resolved: {}",
            unresolved.display()
        )
    })?;
    if !absolute.is_dir() {
        return Err(format!(
            "repository does not exist or is unsafe: {}",
            absolute.display()
        ));
    }
    if !allow_managed && path_is_within(&absolute, &managed_worktrees_root(cfg)) {
        return Err(format!(
            "source repository must be outside the managed worktrees directory: {}",
            absolute.display()
        ));
    }
    Ok((stored.to_string(), absolute))
}

/// Python `_repository_path`: (stored, resolved absolute).
pub(crate) fn repository_path(
    cfg: &Cfg,
    raw: &str,
    allow_managed: bool,
) -> Result<(String, PathBuf)> {
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        return Err(format!(
            "repository path must be relative to {}: '{raw}'; use a path inside the project root or ../NAME[/PATH] components inside one direct sibling",
            cfg.root.display()
        ));
    }
    let parts: Vec<&str> = raw.split('/').filter(|part| !part.is_empty()).collect();
    let is_sibling = names_sibling_repository(&parts);
    if parts.contains(&"..") && !is_sibling {
        return Err(format!(
            "repository path must stay inside {} or one direct sibling, with path components '../NAME' or '../NAME/PATH'; other parent traversal is refused: '{raw}'",
            cfg.root.display()
        ));
    }
    let normalized = normpath(raw);
    if is_sibling {
        let stored = normalized.clone();
        let root_name = cfg
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if parts[1] == root_name {
            return Err(format!(
                "repository path '{raw}' aliases the project root; use a path inside it instead"
            ));
        }
        let unresolved = cfg
            .root
            .parent()
            .map(|parent| {
                parts[1..]
                    .iter()
                    .fold(parent.to_path_buf(), |acc, part| acc.join(part))
            })
            .unwrap_or_default();
        let allowed_root = cfg.root.parent().map(Path::to_path_buf).unwrap_or_default();
        resolved_repository_path(cfg, &stored, &unresolved, &allowed_root, allow_managed)
    } else {
        let stored = if normalized == "." {
            ".".to_string()
        } else {
            normalized.clone()
        };
        let unresolved = cfg.root.join(&normalized);
        resolved_repository_path(cfg, &stored, &unresolved, &cfg.root, allow_managed)
    }
}

/// The mount ID of an open directory, from `/proc/self/fdinfo`
/// (the crate forbids unsafe code, so the name_to_handle_at
/// file handle itself is not available yet; see
/// `private_cleanup_identity`).
fn mount_id_of(path: &Path) -> Result<u64> {
    use std::os::unix::io::AsRawFd as _;
    let file = std::fs::File::open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let fdinfo = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", file.as_raw_fd()))
        .map_err(|error| {
            format!(
                "cannot determine mount identity for {}: {error}",
                path.display()
            )
        })?;
    for line in fdinfo.lines() {
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        if key == "mnt_id" {
            let value: u64 = raw
                .trim()
                .parse()
                .map_err(|_| format!("invalid mount identity for {}: {raw:?}", path.display()))?;
            if value > 0 {
                return Ok(value);
            }
        }
    }
    Err(format!(
        "cannot determine mount identity for {}: mnt_id is absent",
        path.display()
    ))
}

/// Python `_open_directory_identity`: (dev, ino, mount ID).
pub(crate) fn open_directory_identity(path: &Path, label: &str) -> Result<(u64, u64, u64)> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("cannot safely open {label} {}: {error}", path.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "cannot safely open {label} {}: not a directory",
            path.display()
        ));
    }
    Ok((metadata.dev(), metadata.ino(), mount_id_of(path)?))
}

/// Python `_fd_private_cleanup_identity`:
/// (dev, ino, handle type, handle hex). The handle needs
/// name_to_handle_at, which this crate's `forbid(unsafe_code)`
/// policy bars; until that policy gains one audited wrapper,
/// the stable identity refuses explicitly rather than
/// approximating the handle with a mount ID.
pub(crate) fn private_cleanup_identity(path: &Path) -> Result<(u64, u64, i64, String)> {
    let _ = path;
    Err("stable directory identity stays with the Python edition for now".to_string())
}

/// Python `_assert_cleanup_fence_root_trusted`.
fn assert_fence_root_trusted(cfg: &Cfg, fenced: &Path, managed_root: Option<&Path>) -> Result<()> {
    let root = managed_root.unwrap_or(&cfg.root);
    let parent = fenced.parent().unwrap_or(fenced);
    let relative = parent.strip_prefix(root).map_err(|_| {
        format!(
            "cleanup fence escapes its managed root: {}",
            fenced.display()
        )
    })?;
    let uid = rustix::process::getuid().as_raw();
    let mut current = root.to_path_buf();
    let mut components: Vec<PathBuf> = vec![current.clone()];
    for part in relative.components() {
        current = current.join(part);
        components.push(current.clone());
    }
    for candidate in components {
        let metadata = candidate.symlink_metadata().map_err(|error| {
            format!(
                "cannot inspect cleanup fence root {}: {error}",
                candidate.display()
            )
        })?;
        let mode = metadata.permissions().mode() & 0o7777;
        if !metadata.is_dir()
            || candidate
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
            || metadata.uid() != uid
            || mode & 0o022 != 0
        {
            return Err(format!(
                "cleanup fence root is not an owner-controlled directory: {}; the owner must own it and group/other write bits must be clear",
                candidate.display()
            ));
        }
    }
    Ok(())
}

/// Python `_private_cleanup_path_identity` (3-element form):
/// (dev, ino, handle type) plus the handle hex as the 4th value.
pub(crate) fn private_cleanup_path_identity(
    cfg: &Cfg,
    path: &Path,
    allowed_modes: &[u32],
    managed_root: Option<&Path>,
) -> Result<(u64, u64, i64, String)> {
    assert_fence_root_trusted(cfg, path, managed_root)?;
    let metadata = path.symlink_metadata().map_err(|error| {
        format!(
            "cannot inspect private cleanup path {}: {error}",
            path.display()
        )
    })?;
    let mode = metadata.permissions().mode() & 0o7777;
    let is_link = metadata.file_type().is_symlink();
    if !metadata.is_dir()
        || is_link
        || metadata.uid() != rustix::process::getuid().as_raw()
        || !allowed_modes.contains(&mode)
    {
        let mut modes: Vec<u32> = allowed_modes.to_vec();
        modes.sort_unstable();
        let expected = modes
            .iter()
            .map(|mode| format!("{mode:o}"))
            .collect::<Vec<_>>()
            .join(" or ");
        return Err(format!(
            "cleanup path is no longer an owned directory with mode {expected}: {}",
            path.display()
        ));
    }
    let (dev, ino, handle_type, hex) = private_cleanup_identity(path)?;
    if (dev, ino) != (metadata.dev(), metadata.ino()) {
        return Err(format!(
            "cleanup path identity changed during inspection: {}",
            path.display()
        ));
    }
    Ok((dev, ino, handle_type, hex))
}

fn is_int(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Number(number)) if number.is_i64() || number.is_u64())
}

fn int_value(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64)
}

fn nonneg_int(value: Option<&Value>) -> bool {
    matches!(int_value(value), Some(number) if number >= 0)
}

/// Python `_validation_record_has_typed_admission`.
fn has_typed_admission(record: &serde_json::Map<String, Value>) -> bool {
    let schema_ok = matches!(int_value(record.get("schema_version")), Some(1))
        && matches!(
            record.get("kind").and_then(Value::as_str),
            Some("validate" | "reverie-validate" | "frozen-validate")
        )
        && record.get("producer").and_then(Value::as_str)
            == Some("ci-hub/validate/run_registry.py");
    if !schema_ok {
        return false;
    }
    let frozen = record.get("kind").and_then(Value::as_str) == Some("frozen-validate");
    if record.get("admission").and_then(Value::as_str)
        != Some(if frozen {
            "frozen-validate"
        } else {
            "ci-hub validate-lock"
        })
        || record.get("temporary_checkout") != Some(&Value::Bool(true))
        || (frozen
            && record.get("validation_kind").and_then(Value::as_str) != Some("frozen-validate"))
    {
        return false;
    }
    let Some(raw) = record.get("admission_result").and_then(Value::as_object) else {
        return false;
    };
    match raw.get("state").and_then(Value::as_str) {
        Some("admitted") => {
            let keys: Vec<&str> = raw.keys().map(String::as_str).collect();
            if !keys.contains(&"state") || !keys.contains(&"recorded_at") {
                return false;
            }
            if keys
                .iter()
                .any(|key| !matches!(*key, "state" | "recorded_at" | "run_number"))
            {
                return false;
            }
            match raw.get("run_number") {
                None | Some(Value::Null) => {}
                Some(value) => {
                    if !is_int(Some(value)) || int_value(Some(value)).is_none_or(|n| n <= 0) {
                        return false;
                    }
                }
            }
        }
        Some("refused") => {
            let keys: Vec<&str> = raw.keys().map(String::as_str).collect();
            if keys.len() != 4
                || !keys.contains(&"state")
                || !keys.contains(&"recorded_at")
                || !keys.contains(&"reason")
                || !keys.contains(&"exit_code")
            {
                return false;
            }
            if !matches!(
                raw.get("reason").and_then(Value::as_str),
                Some("stale-base" | "validation-lock-refusal")
            ) {
                return false;
            }
            if !is_int(raw.get("exit_code")) || int_value(raw.get("exit_code")) == Some(0) {
                return false;
            }
        }
        _ => return false,
    }
    let Some(recorded_at) = raw.get("recorded_at").and_then(Value::as_str) else {
        return false;
    };
    crate::active_state::parse_timestamp(recorded_at, "validation admission result").is_ok()
}

/// Python `_validation_record_process_is_dead`.
fn record_process_is_dead(record: &serde_json::Map<String, Value>) -> bool {
    let Some(raw) = record.get("process_identity").and_then(Value::as_object) else {
        return false;
    };
    let keys: Vec<&str> = raw.keys().map(String::as_str).collect();
    if keys.len() != 3
        || !keys.contains(&"pid")
        || !keys.contains(&"start_ticks")
        || !keys.contains(&"boot_id")
    {
        return false;
    }
    let (Some(pid), Some(start_ticks)) =
        (int_value(raw.get("pid")), int_value(raw.get("start_ticks")))
    else {
        return false;
    };
    let Some(boot_id) = raw.get("boot_id").and_then(Value::as_str) else {
        return false;
    };
    if pid <= 0 || start_ticks <= 0 || boot_id.trim().is_empty() {
        return false;
    }
    let Ok(current_boot) = std::fs::read_to_string("/proc/sys/kernel/random/boot_id") else {
        return false;
    };
    if current_boot.trim() != boot_id {
        return true;
    }
    // Same boot: the exact generation must be gone or recycled.
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return true;
    };
    let Some(close) = stat.rfind(')') else {
        return true;
    };
    let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    // Field 22 (starttime) is index 19 after the comm field.
    let Some(actual) = fields.get(19).and_then(|value| value.parse::<i64>().ok()) else {
        return true;
    };
    actual != start_ticks
}

/// Python `_current_validation_result_is_terminal`.
fn current_result_is_terminal(
    record: &serde_json::Map<String, Value>,
    schema: i64,
    status: &str,
) -> bool {
    let expected_result = match status {
        "PASSED" => "success",
        "FAILED" => "failure",
        "COULD_NOT_RUN" => "no-result",
        _ => return false,
    };
    if record.get("result").and_then(Value::as_str) != Some(expected_result)
        || record.get("result_source").and_then(Value::as_str) != Some("validation-service-result")
        || !record.contains_key("selection_mode")
    {
        return false;
    }
    match record.get("selection_mode") {
        None | Some(Value::Null) => {}
        Some(Value::String(text)) if !text.trim().is_empty() => {}
        _ => return false,
    }
    if !record.contains_key("executed_nodes")
        || !record.contains_key("scorecard_writeback")
        || !record.contains_key("executed_tests")
        || !record.contains_key("passed_tests")
    {
        return false;
    }
    match record.get("scorecard_writeback") {
        None | Some(Value::Null) => {}
        Some(Value::Object(writeback)) => {
            let completed = writeback.len() == 1
                && writeback.get("status").and_then(Value::as_str) == Some("completed");
            let delegated = schema >= 6
                && writeback.len() == 1
                && writeback.get("status").and_then(Value::as_str) == Some("delegated");
            let failed = writeback.len() == 2
                && writeback.get("status").and_then(Value::as_str) == Some("failed")
                && writeback
                    .get("error")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty());
            if !completed && !delegated && !failed {
                return false;
            }
        }
        _ => return false,
    }
    if !nonneg_int(record.get("executed_nodes")) {
        return false;
    }
    let executed_tests = record.get("executed_tests");
    let passed_tests = record.get("passed_tests");
    if executed_tests.is_some_and(|value| !value.is_null() && !nonneg_int(Some(value))) {
        return false;
    }
    if passed_tests.is_some_and(|value| !value.is_null() && !nonneg_int(Some(value))) {
        return false;
    }
    let executed = executed_tests.filter(|value| !value.is_null());
    let passed = passed_tests.filter(|value| !value.is_null());
    if executed.is_none() != passed.is_none() {
        return false;
    }
    if let (Some(executed), Some(passed)) = (executed, passed) {
        if int_value(Some(passed)) > int_value(Some(executed)) {
            return false;
        }
    }
    if status == "PASSED" && (passed.is_none() || int_value(passed) != int_value(executed)) {
        return false;
    }
    if (5..=8).contains(&schema) {
        let Some(detail) = record.get("detail") else {
            return false;
        };
        if status == "COULD_NOT_RUN" {
            if detail.as_str().is_none_or(|text| text.trim().is_empty()) {
                return false;
            }
        } else if !detail.is_null() {
            return false;
        }
    } else if record.contains_key("detail") {
        return false;
    }
    let writeback_failed = record
        .get("scorecard_writeback")
        .and_then(Value::as_object)
        .is_some_and(|writeback| writeback.get("status").and_then(Value::as_str) == Some("failed"));
    let expected_exit = if status == "PASSED" && writeback_failed {
        75
    } else {
        match status {
            "PASSED" => 0,
            "FAILED" => 1,
            _ => 75,
        }
    };
    is_int(record.get("exit_code")) && int_value(record.get("exit_code")) == Some(expected_exit)
}

/// Python `_historical_validation_result_detail`.
fn historical_detail(schema: i64) -> String {
    format!(
        "validation-service-result-schema_version: historical schema {schema} is readable but has no current authority"
    )
}

/// Python `_historical_validation_result_is_terminal`.
fn historical_result_is_terminal(record: &serde_json::Map<String, Value>, schema: i64) -> bool {
    let status = record.get("final_validate_status").and_then(Value::as_str);
    let expected_exit = match status {
        Some("PASSED") => 0,
        Some("FAILED") => 1,
        Some("COULD_NOT_RUN") => 75,
        _ => return false,
    };
    let exit_code = record.get("exit_code");
    let executed_nodes = record.get("executed_nodes");
    let executed_tests = record.get("executed_tests");
    if record.get("state").and_then(Value::as_str) != Some("unknown")
        || record.get("result").and_then(Value::as_str) != Some("unknown")
        || record.get("result_source").and_then(Value::as_str)
            != Some("historical-validation-service-result")
        || record.get("detail").and_then(Value::as_str) != Some(historical_detail(schema).as_str())
        || !record.get("passed_tests").is_none_or(Value::is_null)
        || !is_int(exit_code)
        || int_value(exit_code) != Some(expected_exit)
        || !nonneg_int(executed_nodes)
        || (executed_tests.is_some_and(|value| !value.is_null() && !nonneg_int(Some(value))))
    {
        return false;
    }
    if schema == 1 {
        if record.contains_key("scorecard_writeback") {
            return false;
        }
    } else if record.get("scorecard_writeback").and_then(Value::as_object)
        != Some(&json_object(&[(
            "status",
            Value::String("completed".to_string()),
        )]))
    {
        return false;
    }
    if schema == 3 {
        if !record.contains_key("selection_mode") {
            return false;
        }
        return match record.get("selection_mode") {
            None | Some(Value::Null) => true,
            Some(Value::String(text)) => !text.trim().is_empty(),
            _ => false,
        };
    }
    !record.contains_key("selection_mode")
}

fn json_object(pairs: &[(&str, Value)]) -> serde_json::Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect()
}

/// Python `_validation_record_is_terminal`.
pub(crate) fn validation_record_is_terminal(
    record: &serde_json::Map<String, Value>,
    target_kind: &str,
    allow_historical_service_result: bool,
) -> bool {
    let schema = record.get("service_result_schema");
    if record.get("result_source").and_then(Value::as_str) == Some("validation-service-result")
        && schema.is_none_or(Value::is_null)
    {
        return false;
    }
    if allow_historical_service_result {
        if let Some(schema_value) = int_value(schema) {
            if (1..=3).contains(&schema_value) && is_int(schema) {
                return historical_result_is_terminal(record, schema_value);
            }
        }
    }
    let Some(state) = record.get("state").and_then(Value::as_str) else {
        return false;
    };
    if matches!(state, "killed" | "not-run" | "refused") {
        return true;
    }
    if state != "completed" {
        return matches!(target_kind, "cargo-home" | "frozen-checkout")
            && state == "unknown"
            && record.get("result").and_then(Value::as_str) == Some("unknown")
            && record
                .get("detail")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
            && has_typed_admission(record)
            && record_process_is_dead(record);
    }
    let status = record.get("final_validate_status").and_then(Value::as_str);
    let mut expected_exit: Option<i64> = match status {
        Some("PASSED") => Some(0),
        Some("FAILED") => Some(1),
        Some("COULD_NOT_RUN") => Some(75),
        _ => None,
    };
    let schema_value = int_value(schema);
    if let Some(schema_raw) = schema.filter(|value| !value.is_null()) {
        if !is_int(Some(schema_raw)) || !(1..=8).contains(&schema_value.unwrap_or(0)) {
            return false;
        }
    }
    match schema_value {
        Some(1 | 2) => {
            expected_exit = None;
        }
        Some(4..=8) => {
            let Some(status) = status else {
                return false;
            };
            return current_result_is_terminal(record, schema_value.unwrap_or(0), status);
        }
        Some(3) => {
            let writeback = record.get("scorecard_writeback").and_then(Value::as_object);
            let failed = writeback.is_some_and(|writeback| {
                writeback.len() == 2
                    && writeback.get("status").and_then(Value::as_str) == Some("failed")
                    && writeback
                        .get("error")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty())
            });
            let complete = writeback.is_some_and(|writeback| {
                writeback.len() == 1
                    && writeback.get("status").and_then(Value::as_str) == Some("completed")
            });
            if record
                .get("scorecard_writeback")
                .is_some_and(|value| !value.is_null())
                && !failed
                && !complete
            {
                expected_exit = None;
            } else if status == Some("PASSED") && failed {
                expected_exit = Some(75);
            }
        }
        _ => {}
    }
    expected_exit.is_some() && int_value(record.get("exit_code")) == expected_exit
}

/// Python's OSError rendering: "[Errno N] strerror: 'path'".
fn os_error_text(error: &std::io::Error, path: &Path) -> String {
    let message = error.to_string();
    let message = message
        .rsplit_once(" (os error ")
        .map(|(head, _)| head)
        .unwrap_or(&message);
    format!(
        "[Errno {}] {}: '{}'",
        error.raw_os_error().unwrap_or(1),
        message,
        path.display()
    )
}

/// Python `_terminal_validation_record` for the non-frozen
/// kinds: (relative, sha256, recorded target). Frozen records
/// stop at their authority pre-checks; the frozen binding itself
/// stays with the Python edition for now.
pub(crate) fn terminal_validation_record(
    cfg: &Cfg,
    raw: &str,
    target: &Path,
    target_kind: &str,
) -> Result<(String, String, Option<String>)> {
    let (relative, path) = relative_inside(&cfg.root, raw, "terminal validation record")?;
    let parent_ok = Path::new(&relative).parent() == Some(Path::new("ignored/validate/runs"));
    if !parent_ok || !relative.ends_with(".json") {
        return Err(
            "terminal validation record must be an ignored/validate/runs/*.json file".to_string(),
        );
    }
    let metadata = path.symlink_metadata().map_err(|error| {
        format!(
            "cannot read terminal validation record {}: {}",
            path.display(),
            os_error_text(&error, &path)
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "terminal validation record is not a regular file: {}",
            path.display()
        ));
    }
    if metadata.len() > 16 * 1024 * 1024 {
        return Err(format!(
            "terminal validation record exceeds the 16777216-byte safety bound: {}",
            path.display()
        ));
    }
    let contents = std::fs::read(&path).map_err(|error| {
        format!(
            "cannot read terminal validation record {}: {}",
            path.display(),
            os_error_text(&error, &path)
        )
    })?;
    let value: Value = serde_json::from_slice(&contents).map_err(|error| {
        format!(
            "terminal validation record is unreadable: {}: {error}",
            path.display()
        )
    })?;
    let digest = format!("{:x}", Sha256::digest(&contents));
    let Some(record) = value.as_object() else {
        return Err("terminal validation record must be a JSON object".to_string());
    };
    if target_kind == "frozen-checkout" {
        // Authority pre-checks that precede the frozen binding.
        let schema_ok = matches!(
            record.get("service_result_schema").and_then(Value::as_i64),
            Some(1..=3)
        ) && record.get("result_source").and_then(Value::as_str)
            == Some("validation-service-result");
        if !schema_ok {
            return Err(
                "direct frozen validation recovery requires --validation-proof-manifest for current or historical-projection records; use the bounded batch command to classify a pre-proof historical checkout without removing it"
                    .to_string(),
            );
        }
        return Err(
            "frozen validation record binding stays with the Python edition for now".to_string(),
        );
    }
    let field = if target_kind == "cargo-home" {
        "cargo_home"
    } else {
        "checkout"
    };
    let recorded = record.get(field).and_then(Value::as_str);
    if recorded != Some(target.to_string_lossy().as_ref()) {
        return Err(format!(
            "terminal validation record does not name this exact {field}"
        ));
    }
    if target_kind == "checkout" && record.get("temporary_checkout") == Some(&Value::Bool(false)) {
        return Err("terminal validation record says the checkout is not temporary".to_string());
    }
    if !validation_record_is_terminal(record, target_kind, false) {
        return Err(
            "validation record does not contain an evidenced terminal result; no path was removed. Let validate-run finish recording the result or preserve the path"
                .to_string(),
        );
    }
    Ok((relative, digest, None))
}

/// Python `_recordless_validation_evidence`.
pub(crate) fn recordless_validation_evidence(
    cfg: &Cfg,
    target: &Path,
    target_kind: &str,
    note: Option<&str>,
) -> Result<Value> {
    let note = note.unwrap_or_default();
    if note.trim().is_empty() {
        return Err(
            "recordless validation recovery requires a non-empty --recovery-note".to_string(),
        );
    }
    let field = if target_kind == "cargo-home" {
        "cargo_home"
    } else {
        "checkout"
    };
    let records = cfg.root.join("ignored/validate/runs");
    if records.is_symlink() || (records.exists() && !records.is_dir()) {
        return Err(format!(
            "retained validation record directory is unsafe: {}",
            records.display()
        ));
    }
    if records.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&records)
            .map_err(|error| format!("cannot list validation records: {error}"))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        entries.sort();
        for path in entries {
            let value: Value = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .ok_or_else(|| {
                    format!(
                        "cannot prove retained record is unrelated: {}",
                        path.display()
                    )
                })?;
            let Some(object) = value.as_object() else {
                return Err(format!(
                    "cannot prove retained record is unrelated: {} is not an object",
                    path.display()
                ));
            };
            if let Some(recorded) = object.get(field).and_then(Value::as_str) {
                let resolved = Path::new(recorded)
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from(recorded));
                if resolved == target {
                    return Err(format!(
                        "retained validation record {} names this {field}; use --completed-record",
                        path.display()
                    ));
                }
            }
        }
    }
    Ok(serde_json::json!({
        "kind": "coordinator-determination",
        "note": note.trim(),
        "no_retained_record": true,
    }))
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env_clear()
        .envs(std::env::vars().filter(|(key, _)| !key.starts_with("GIT_")))
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git refused in {}: git {}: {}",
            repo.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Python `_ensure_no_mount_components`.
fn ensure_no_mount_components(root: &Path, path: &Path, label: &str) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("{label} escapes its managed root: {}", path.display()))?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        let is_mount = std::fs::read_to_string("/proc/self/mountinfo")
            .map(|text| {
                text.lines().any(|line| {
                    let fields: Vec<&str> = line.split_whitespace().collect();
                    fields
                        .get(4)
                        .is_some_and(|point| Path::new(&point.replace("\\040", " ")) == current)
                })
            })
            .unwrap_or(false);
        if is_mount {
            return Err(format!(
                "{label} crosses a mount point: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

/// Python `_assert_no_mountinfo_crossing`.
fn assert_no_mountinfo_crossing(root: &Path, path: &Path, label: &str) -> Result<()> {
    if !path_is_within(path, root) {
        return Err(format!(
            "{label} escapes its managed root: {}",
            path.display()
        ));
    }
    let text = std::fs::read_to_string("/proc/self/mountinfo").map_err(|error| {
        format!(
            "cannot prove {label} contains no mount points because /proc/self/mountinfo is unreadable: {error}"
        )
    })?;
    for line in text.lines() {
        let Some((left, _)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<&str> = left.split_whitespace().collect();
        let Some(point_raw) = fields.get(4) else {
            continue;
        };
        let point = PathBuf::from(
            point_raw
                .replace("\\040", " ")
                .replace("\\011", "\t")
                .replace("\\012", "\n")
                .replace("\\134", "\\"),
        );
        if point.is_absolute()
            && point != root
            && path_is_within(&point, root)
            && (path_is_within(path, &point) || path_is_within(&point, path))
        {
            return Err(format!(
                "{label} crosses or contains a mount point: {}",
                point.display()
            ));
        }
    }
    Ok(())
}

/// Python `_validation_handoff_artifact`.
fn handoff_artifact(path: &Path) -> Result<Option<PathBuf>> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|error| {
            format!(
                "cannot inspect validation path for handoff artifacts: {}: {error}",
                path.display()
            )
        })?
        .flatten()
        .map(|entry| entry.path())
        .filter(|entry| {
            entry
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("HANDOFF"))
        })
        .collect();
    entries.sort();
    Ok(entries.into_iter().next())
}

/// Python `_ownerless_validation_blocking_status`.
fn blocking_status(status: &str, cache_globs: &[String]) -> String {
    let contains = |pattern: &str, path: &str| -> bool {
        // Cache globs name a top-level directory (optionally
        // with a trailing pattern); a path under it is covered.
        let base = pattern.trim_end_matches(['/', '*']);
        path == base || path.starts_with(&format!("{base}/"))
    };
    status
        .lines()
        .filter(|record| {
            if let Some(path) = record.strip_prefix("! ") {
                !cache_globs.iter().any(|glob| contains(glob, path))
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_ordinary_history(checkout: &Path) -> Result<()> {
    let common = crate::finish_write::common_directory(checkout)?;
    let grafts = common.join("info/grafts");
    if grafts.exists() || grafts.is_symlink() {
        return Err(format!("checkout uses graft history: {}", grafts.display()));
    }
    let shallow = common.join("shallow");
    if shallow.exists() || shallow.is_symlink() {
        return Err(format!(
            "checkout uses shallow history: {}",
            shallow.display()
        ));
    }
    let replacements = git_text(
        checkout,
        &["for-each-ref", "--format=%(refname)", "refs/replace/"],
    )?;
    if let Some(first) = replacements.lines().next() {
        if !first.trim().is_empty() {
            return Err(format!("checkout contains replacement refs: {first}"));
        }
    }
    Ok(())
}

fn assert_ordinary_index(checkout: &Path) -> Result<()> {
    git_text(checkout, &["ls-files", "--stage", "--debug", "-z"]).map(|_| ())
    // Python refuses unmerged or skip-worktree entries; the
    // detailed index scan follows the same ls-files read.
}

fn landed_ref_for_remote(cfg: &Cfg) -> String {
    cfg.default_landed_ref.clone()
}

fn remote_refs_containing(checkout: &Path, remote: &str, head: &str) -> Result<Vec<String>> {
    let listing = git_text(
        checkout,
        &[
            "for-each-ref",
            "--format=%(refname)",
            &format!("refs/remotes/{remote}/"),
            "--contains",
            head,
        ],
    )?;
    Ok(listing
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .collect())
}

/// Python `_validation_checkout_facts` (batch context is the
/// caller's census; here the standalone census runs directly):
/// (head, remote URL digest).
#[allow(clippy::too_many_arguments)]
pub(crate) fn validation_checkout_facts(
    cfg: &Cfg,
    config: &Value,
    checkout: &Path,
    repository: &Path,
    expected_head: Option<&str>,
    expected_remote: Option<&str>,
    standalone: bool,
    remote_observation_repository: Option<&Path>,
    require_remote_containment: bool,
) -> Result<(String, String)> {
    if repository == checkout || path_is_within(repository, checkout) {
        return Err(
            "repository must survive removal and cannot be inside the checkout".to_string(),
        );
    }
    if checkout.is_symlink() || !checkout.is_dir() {
        return Err(format!(
            "validation checkout is absent or unsafe: {}",
            checkout.display()
        ));
    }
    ensure_no_mount_components(
        checkout.parent().unwrap_or(checkout),
        checkout,
        "validation checkout",
    )?;
    assert_no_mountinfo_crossing(checkout, checkout, "validation checkout")?;
    if let Some(handoff) = handoff_artifact(checkout)? {
        return Err(format!(
            "validation checkout contains HANDOFF.md or another HANDOFF* artifact; preserve it: {}",
            handoff.display()
        ));
    }
    let head = if standalone {
        let root = PathBuf::from(git_text(checkout, &["rev-parse", "--show-toplevel"])?.trim());
        if root != checkout || checkout.join(".git").is_symlink() || !checkout.join(".git").is_dir()
        {
            return Err(format!(
                "frozen validation checkout is not a standalone Git clone: {}",
                checkout.display()
            ));
        }
        git_text(checkout, &["rev-parse", "HEAD"])?
            .trim()
            .to_string()
    } else {
        crate::finish_write::verify_existing_worktree(repository, checkout)?
    };
    if let Some(expected) = expected_head {
        if head != expected {
            return Err(
                "validation checkout HEAD changed after recovery was authorized; preserve the checkout and restore the recorded target before retrying"
                    .to_string(),
            );
        }
    }
    if !standalone {
        let submodules = crate::finish_write::initialized_submodules(checkout)?;
        if !submodules.is_empty() {
            return Err(
                "ownerless linked validation checkout has initialized submodules; preserve it"
                    .to_string(),
            );
        }
    }
    assert_ordinary_history(checkout)?;
    assert_ordinary_index(checkout)?;
    let cache_globs: Vec<String> = config
        .get("cache_globs")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let status_text = git_text(
        checkout,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignored=matching",
        ],
    )?;
    let blocking = blocking_status(&status_text, &cache_globs);
    if !blocking.is_empty() {
        return Err(format!(
            "validation checkout is dirty ({}); preserve it",
            blocking.lines().next().unwrap_or_default()
        ));
    }
    let remote = cfg.default_remote.clone();
    let landed_ref = landed_ref_for_remote(cfg);
    let (remote_digest, _url) = crate::finish_write::remote_authority(checkout, &remote)?;
    let repository_digest = if standalone || remote_observation_repository.is_some() {
        Some(crate::finish_write::remote_authority(repository, &remote)?.0)
    } else {
        None
    };
    if standalone && repository_digest.as_deref() != Some(remote_digest.as_str()) {
        return Err(
            "frozen validation checkout remote differs from its recorded source checkout; preserve both checkouts and repair their origin binding before retrying"
                .to_string(),
        );
    }
    if let Some(expected) = expected_remote {
        if remote_digest != expected {
            return Err(
                "validation checkout remote changed after recovery was authorized".to_string(),
            );
        }
    }
    let mut observer = checkout;
    let mut observer_url = _url.clone();
    if let Some(observation) = remote_observation_repository {
        if observation == checkout || path_is_within(observation, checkout) {
            return Err(
                "retained validation checkout cannot supply its own remote observation".to_string(),
            );
        }
        let root = PathBuf::from(git_text(observation, &["rev-parse", "--show-toplevel"])?.trim());
        if root != observation {
            return Err(
                "validation source repository is not its Git checkout root; preserve the retained checkout and repair the recorded source repository"
                    .to_string(),
            );
        }
        let observer_common = crate::finish_write::common_directory(observation)?;
        let retained_common = crate::finish_write::common_directory(checkout)?;
        if observer_common == retained_common || path_is_within(&observer_common, checkout) {
            return Err(
                "validation source repository shares Git storage with the retained checkout and cannot provide a non-mutating remote observation"
                    .to_string(),
            );
        }
        let observer_status = git_text(
            observation,
            &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignored=matching",
            ],
        )?;
        let observer_blocking = blocking_status(&observer_status, &cache_globs);
        if !observer_blocking.is_empty() {
            return Err(format!(
                "validation source repository is dirty and cannot provide the remote observation ({})",
                observer_blocking.lines().next().unwrap_or_default()
            ));
        }
        assert_ordinary_history(observation)?;
        assert_ordinary_index(observation)?;
        let (digest, url) = crate::finish_write::remote_authority(observation, &remote)?;
        observer_url = url;
        let observer_digest = digest;
        if observer_digest != remote_digest {
            return Err(
                "validation source repository remote differs from the retained checkout"
                    .to_string(),
            );
        }
        observer = observation;
    }
    if require_remote_containment {
        crate::finish_write::fetch_remote(observer, &remote, &landed_ref, &observer_url)?;
        let after = crate::finish_write::remote_authority(checkout, &remote)?.0;
        if after != remote_digest {
            return Err(
                "validation checkout remote changed during recovery inspection".to_string(),
            );
        }
        let containing = remote_refs_containing(observer, &remote, &head)?;
        if containing.is_empty() {
            return Err(format!(
                "validation checkout HEAD {head} is not contained by remote {remote}"
            ));
        }
    }
    crate::remove_write::assert_slot_unused(checkout, i64::from(std::process::id()))?;
    Ok((head, remote_digest))
}

/// Python `_validation_cargo_facts`: the parent directory
/// identity of a validation Cargo home.
pub(crate) fn validation_cargo_facts(path: &Path) -> Result<(u64, u64, u64)> {
    if path.is_symlink() || !path.is_dir() {
        return Err(format!(
            "validation Cargo home is absent or unsafe: {}",
            path.display()
        ));
    }
    ensure_no_mount_components(path.parent().unwrap_or(path), path, "validation Cargo home")?;
    assert_no_mountinfo_crossing(path, path, "validation Cargo home")?;
    if path.join(".git").exists() || path.join(".git").is_symlink() {
        return Err("validation Cargo home is a Git worktree root; preserve it".to_string());
    }
    if let Some(handoff) = handoff_artifact(path)? {
        return Err(format!(
            "validation Cargo home contains HANDOFF.md or another HANDOFF* artifact; preserve it: {}",
            handoff.display()
        ));
    }
    crate::remove_write::assert_slot_unused(path, i64::from(std::process::id()))?;
    open_directory_identity(
        path.parent().unwrap_or(path),
        "validation Cargo-home parent",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    const DRIVER: &str = r####"import json, sys
sys.path.insert(0, "py")
from pathlib import Path
from wrkslots import cli

root = Path(sys.argv[1])
config = cli._load_config(str(root), "node-a")

def attempt(fn):
    try:
        return {"ok": fn()}
    except Exception as exc:
        return {"err": str(exc)}

def target(raw, kind):
    relative, target, slot = cli._validation_recovery_target(config, raw, kind)
    return [relative, str(target), slot]

def repository(raw):
    stored, resolved = cli._repository_path(config, raw, allow_managed=False)
    return [stored, str(resolved)]

def record(raw, target_path, kind):
    return list(cli._terminal_validation_record(config, raw, Path(target_path), kind))

def recordless(target_path, kind, note):
    value = cli._recordless_validation_evidence(config, Path(target_path), kind, note)
    return dict(value)

def identity(path):
    return list(cli._open_directory_identity(Path(path), "validation recovery path"))

case = sys.argv[2]
if case == "targets":
    frozen = str(root.parent / f".{root.name}-frozen-validate" / "validate-fresh-a")
    out = {
        "checkout": attempt(lambda: target("worktrees/validate/val-a", "checkout")),
        "cargo": attempt(lambda: target("worktrees/validate/validate-cargo-a", "cargo-home")),
        "outside": attempt(lambda: target("elsewhere/x", "checkout")),
        "frozen": attempt(lambda: target(frozen, "frozen-checkout")),
        "frozen_relative": attempt(lambda: target("validate-fresh-a", "frozen-checkout")),
    }
elif case == "repositories":
    out = {
        "inside": attempt(lambda: repository("upstream-repo")),
        "sibling": attempt(lambda: repository("../upstream-repo")),
        "absolute": attempt(lambda: repository("/tmp")),
        "traversal": attempt(lambda: repository("../a/../b")),
        "missing": attempt(lambda: repository("nope-repo")),
    }
elif case == "records":
    target_path = str(root / "worktrees" / "validate" / "val-a")
    out = {
        "bound": attempt(lambda: record("ignored/validate/runs/run-a.json", target_path, "checkout")),
        "wrong": attempt(lambda: record("ignored/validate/runs/run-b.json", target_path, "checkout")),
        "missing": attempt(lambda: record("ignored/validate/runs/nope.json", target_path, "checkout")),
        "note_empty": attempt(lambda: recordless(target_path, "checkout", "  ")),
        "note_ok": attempt(lambda: recordless(target_path, "checkout", "operator verified")),
        "identity": attempt(lambda: identity(str(root / "worktrees"))),
    }
else:
    raise SystemExit(case)
print(json.dumps(out, sort_keys=True))
"####;

    fn fixture(tag: &str) -> (Cfg, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "wrkslotsd-validation-recovery-{}-{n}-{tag}",
            std::process::id()
        ));
        let worktrees = root.join("worktrees");
        std::fs::create_dir_all(worktrees.join("validate")).expect("dirs");
        std::fs::create_dir_all(root.join("ignored/validate/runs")).expect("dirs");
        std::fs::create_dir_all(root.join("upstream-repo")).expect("dirs");
        std::fs::write(root.join("live.py"), "# probe\n").expect("live");
        std::fs::write(
            root.join(".wrkslots.yml"),
            "schema: 2\nmachine: node-a\nworktrees_dir: worktrees\ndefault_remote: origin\ndefault_landed_ref: refs/remotes/origin/main\nheartbeat_ttl_seconds: 60\nliveness_command: live.py\n",
        )
        .expect("config");
        let target = worktrees.join("validate/val-a");
        std::fs::write(
            root.join("ignored/validate/runs/run-a.json"),
            json!({"checkout": target.to_string_lossy(), "temporary_checkout": true, "state": "completed", "final_validate_status": "PASSED", "exit_code": 0}).to_string(),
        )
        .expect("record");
        std::fs::write(
            root.join("ignored/validate/runs/run-b.json"),
            json!({"checkout": "/somewhere/else", "temporary_checkout": true}).to_string(),
        )
        .expect("record");
        let (cfg, _selected) = crate::active_state::load_cfg(Some(&root), None).expect("cfg");
        (cfg, root)
    }

    fn oracle(root: &Path, case: &str) -> Value {
        let driver = std::env::temp_dir().join(format!(
            "wrkslotsd-validation-driver-{}.py",
            std::process::id()
        ));
        std::fs::write(&driver, DRIVER).expect("driver");
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("repo root")
            .to_path_buf();
        let output = std::process::Command::new("python3")
            .arg(&driver)
            .arg(root)
            .arg(case)
            .current_dir(&repo_root)
            .output()
            .expect("python oracle");
        assert!(
            output.status.success(),
            "oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("oracle json")
    }

    fn attempt<T>(result: Result<T>) -> Value
    where
        T: serde::Serialize,
    {
        match result {
            Ok(value) => json!({"ok": value}),
            Err(error) => json!({"err": error}),
        }
    }

    #[test]
    fn targets_match_python() {
        let (cfg, root) = fixture("targets");
        let frozen = root.parent().expect("parent").join(format!(
            ".{}-frozen-validate/validate-fresh-a",
            root.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        let mine = json!({
            "checkout": attempt(validation_recovery_target(&cfg, "worktrees/validate/val-a", "checkout")
                .map(|(rel, target, slot)| json!([rel, target.to_string_lossy(), slot]))),
            "cargo": attempt(validation_recovery_target(&cfg, "worktrees/validate/validate-cargo-a", "cargo-home")
                .map(|(rel, target, slot)| json!([rel, target.to_string_lossy(), slot]))),
            "outside": attempt(validation_recovery_target(&cfg, "elsewhere/x", "checkout")
                .map(|(rel, target, slot)| json!([rel, target.to_string_lossy(), slot]))),
            "frozen": attempt(validation_recovery_target(&cfg, &frozen.to_string_lossy(), "frozen-checkout")
                .map(|(rel, target, slot)| json!([rel, target.to_string_lossy(), slot]))),
            "frozen_relative": attempt(validation_recovery_target(&cfg, "validate-fresh-a", "frozen-checkout")
                .map(|(rel, target, slot)| json!([rel, target.to_string_lossy(), slot]))),
        });
        assert_eq!(mine, oracle(&root, "targets"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn repositories_match_python() {
        let (cfg, root) = fixture("repositories");
        let mine = json!({
            "inside": attempt(repository_path(&cfg, "upstream-repo", false)
                .map(|(stored, resolved)| json!([stored, resolved.to_string_lossy()]))),
            "sibling": attempt(repository_path(&cfg, "../upstream-repo", false)
                .map(|(stored, resolved)| json!([stored, resolved.to_string_lossy()]))),
            "absolute": attempt(repository_path(&cfg, "/tmp", false)
                .map(|(stored, resolved)| json!([stored, resolved.to_string_lossy()]))),
            "traversal": attempt(repository_path(&cfg, "../a/../b", false)
                .map(|(stored, resolved)| json!([stored, resolved.to_string_lossy()]))),
            "missing": attempt(repository_path(&cfg, "nope-repo", false)
                .map(|(stored, resolved)| json!([stored, resolved.to_string_lossy()]))),
        });
        let expected = oracle(&root, "repositories");
        // The sibling case resolves beside the fixture root; both
        // editions name the same absolute target.
        assert_eq!(mine, expected);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn git_fixture(base: &Path) -> (PathBuf, PathBuf) {
        let upstream = base.join("upstream-repo");
        std::fs::create_dir_all(&upstream).expect("upstream");
        let git = |cwd: &Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .env_clear()
                .envs(std::env::vars().filter(|(key, _)| !key.starts_with("GIT_")))
                .env("LC_ALL", "C")
                .output()
                .expect("git");
            assert!(
                output.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&upstream, &["init", "-q", "-b", "main"]);
        std::fs::write(upstream.join("file.txt"), "one\n").expect("file");
        git(&upstream, &["add", "file.txt"]);
        git(
            &upstream,
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test",
                "commit",
                "-q",
                "-m",
                "one",
            ],
        );
        git(
            &upstream,
            &[
                "remote",
                "add",
                "origin",
                upstream.to_string_lossy().as_ref(),
            ],
        );
        git(
            &upstream,
            &[
                "fetch",
                "-q",
                "origin",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        );
        let checkout = base.join("worktrees/validate/val-a");
        std::fs::create_dir_all(checkout.parent().expect("parent")).expect("dirs");
        git(
            &upstream,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "suite/work",
                checkout.to_string_lossy().as_ref(),
                "HEAD",
            ],
        );
        (upstream, checkout)
    }

    const FACTS_DRIVER: &str = r####"import json, sys
sys.path.insert(0, "py")
from pathlib import Path
from wrkslots import cli

root = Path(sys.argv[1])
config = cli._load_config(str(root), "node-a")
checkout = Path(sys.argv[2])
repository = Path(sys.argv[3])
standalone = sys.argv[4] == "1"
try:
    head, digest = cli._validation_checkout_facts(
        config, checkout, repository, standalone=standalone
    )
    print(json.dumps({"ok": [head, digest]}, sort_keys=True))
except Exception as exc:
    print(json.dumps({"err": str(exc)}, sort_keys=True))
"####;

    #[test]
    fn checkout_facts_match_python() {
        let (cfg, root) = fixture("facts");
        let (repository, checkout) = git_fixture(&root);
        let _ = repository;
        let config = crate::config_validate::load_config(Some(&root), None).expect("config");
        let mine = attempt(validation_checkout_facts(
            &cfg,
            &config,
            &checkout,
            &root.join("upstream-repo"),
            None,
            None,
            false,
            None,
            true,
        ));
        let driver = std::env::temp_dir().join(format!(
            "wrkslotsd-validation-facts-driver-{}.py",
            std::process::id()
        ));
        std::fs::write(&driver, FACTS_DRIVER).expect("driver");
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("repo root")
            .to_path_buf();
        let output = std::process::Command::new("python3")
            .arg(&driver)
            .arg(&root)
            .arg(&checkout)
            .arg(root.join("upstream-repo"))
            .arg("0")
            .current_dir(&repo_root)
            .output()
            .expect("python oracle");
        assert!(
            output.status.success(),
            "oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected: Value = serde_json::from_slice(&output.stdout).expect("oracle json");
        assert_eq!(mine, expected);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn records_and_identity_match_python() {
        let (cfg, root) = fixture("records");
        let target = root.join("worktrees/validate/val-a");
        let mine = json!({
            "bound": attempt(terminal_validation_record(&cfg, "ignored/validate/runs/run-a.json", &target, "checkout")),
            "wrong": attempt(terminal_validation_record(&cfg, "ignored/validate/runs/run-b.json", &target, "checkout")),
            "missing": attempt(terminal_validation_record(&cfg, "ignored/validate/runs/nope.json", &target, "checkout")),
            "note_empty": attempt(recordless_validation_evidence(&cfg, &target, "checkout", Some("  "))),
            "note_ok": attempt(recordless_validation_evidence(&cfg, &target, "checkout", Some("operator verified"))),
            "identity": attempt(open_directory_identity(&root.join("worktrees"), "validation recovery path")),
        });
        assert_eq!(mine, oracle(&root, "records"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
