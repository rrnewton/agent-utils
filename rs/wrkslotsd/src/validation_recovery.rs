//! Step 1a of the ownerless-validation port (#1088): target
//! resolution, repository paths, directory identities, and the
//! terminal validation record's non-frozen binding, mirroring
//! the Python helpers of the same names. The frozen record
//! binding and checkout facts land in later steps; paths that
//! need them refuse explicitly.

use std::os::unix::fs::MetadataExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
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

fn service_result_fields(schema: i64) -> Option<Vec<&'static str>> {
    let base = [
        "schema_version",
        "commit",
        "profile",
        "final_validate_status",
        "exit_code",
        "executed_nodes",
        "executed_tests",
    ];
    let fields: Vec<&str> = match schema {
        1 => base.to_vec(),
        2 => [base.as_slice(), &["scorecard_writeback"]].concat(),
        3 => [
            &base[..4],
            &["selection_mode"],
            &base[4..],
            &["scorecard_writeback"],
        ]
        .concat(),
        4 => [
            &base[..4],
            &["selection_mode"],
            &base[4..],
            &["passed_tests", "scorecard_writeback"],
        ]
        .concat(),
        5 | 6 => [
            &base[..4],
            &["selection_mode", "detail"],
            &base[4..],
            &["passed_tests", "scorecard_writeback"],
        ]
        .concat(),
        7 => [
            &base[..4],
            &["selection_mode", "detail"],
            &base[4..],
            &[
                "passed_tests",
                "diagnostic_failed_tests",
                "scorecard_writeback",
            ],
        ]
        .concat(),
        8 => [
            &base[..4],
            &["selection_mode", "detail"],
            &base[4..],
            &[
                "passed_tests",
                "diagnostic_failed_tests",
                "skipped_tests",
                "scorecard_writeback",
            ],
        ]
        .concat(),
        _ => return None,
    };
    Some(fields)
}

fn strict_json_object(contents: &[u8], label: &str) -> Result<serde_json::Map<String, Value>> {
    let value: Value = serde_json::from_slice(contents)
        .map_err(|error| format!("{label} is malformed JSON: {error}"))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{label} must be a JSON object"))
}

/// Python `_validation_service_result_schema`: the declared
/// fields when the producer schema matches, else a refusal.
fn service_result_schema(contents: &[u8], schema: i64) -> Result<Vec<&'static str>> {
    let producer = strict_json_object(contents, "validation removal producer-schema")?;
    let fields = service_result_fields(schema).ok_or_else(|| {
        "validation removal producer-schema does not match the run-record schema".to_string()
    })?;
    let mut expected_keys: Vec<&str> = vec!["schema_version", "fields", "outcomes"];
    if schema >= 2 {
        expected_keys.push("scorecard_writeback");
    }
    let exit_field = if schema == 1 {
        "exit_code"
    } else {
        "validation_exit_code"
    };
    let outcome =
        |exit: i64, result: &str| json!({exit_field: exit, "state": "completed", "result": result});
    let mut outcomes = json!({
        "PASSED": outcome(0, "success"),
        "FAILED": outcome(1, "failure"),
        "COULD_NOT_RUN": outcome(75, "no-result"),
    });
    if schema >= 7 {
        outcomes
            .as_object_mut()
            .expect("object")
            .insert("GREEN_FLAKY".to_string(), outcome(0, "success"));
    }
    let base_writeback = json!({
        "nullable": true,
        "variants": {"completed": ["status"], "failed": ["status", "error"]},
    });
    let expected_writeback = if schema >= 6 {
        json!({
            "nullable": true,
            "variants": {
                "completed": ["status"],
                "delegated": ["status"],
                "failed": ["status", "error"],
            },
        })
    } else {
        base_writeback
    };
    let keys: Vec<&str> = producer.keys().map(String::as_str).collect();
    let fields_match = producer
        .get("fields")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                == fields
                    .iter()
                    .map(|item| item.to_string())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
        });
    if keys.len() != expected_keys.len()
        || !expected_keys.iter().all(|key| keys.contains(key))
        || !is_int(producer.get("schema_version"))
        || int_value(producer.get("schema_version")) != Some(schema)
        || !fields_match
        || producer.get("outcomes") != Some(&outcomes)
        || (schema >= 2 && producer.get("scorecard_writeback") != Some(&expected_writeback))
    {
        return Err(
            "validation removal producer-schema does not match the run-record schema".to_string(),
        );
    }
    Ok(fields)
}

fn scorecard_writeback(value: Option<&Value>, label: &str) -> Result<Option<Value>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(object) = value.as_object() else {
        return Err(format!("{label} must be an object or null"));
    };
    let keys: Vec<&str> = object.keys().map(String::as_str).collect();
    if object.get("status").and_then(Value::as_str) == Some("completed") && keys == ["status"] {
        return Ok(Some(value.clone()));
    }
    let error = object.get("error").and_then(Value::as_str);
    if object.get("status").and_then(Value::as_str) == Some("failed")
        && keys.len() == 2
        && keys.contains(&"status")
        && keys.contains(&"error")
        && error.is_some_and(|text| !text.trim().is_empty())
    {
        return Ok(Some(value.clone()));
    }
    Err(format!(
        "{label} must be null, completed, or failed with a nonempty error"
    ))
}

fn optional_count(value: Option<&Value>, label: &str) -> Result<Option<i64>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(other) => {
            if is_int(Some(other)) && int_value(Some(other)).is_some_and(|n| n >= 0) {
                Ok(int_value(Some(other)))
            } else {
                Err(format!("{label} must be a non-negative integer or null"))
            }
        }
    }
}

/// Python `_validation_service_result_projection`.
fn service_result_projection(
    contents: &[u8],
    schema: i64,
    schema_fields: &[&str],
    expected_commit: Option<&Value>,
) -> Result<serde_json::Map<String, Value>> {
    let result = strict_json_object(contents, "validation removal service-result")?;
    let mut allowed: Vec<&str> = schema_fields.to_vec();
    if schema >= 8 {
        allowed.push("test_cases");
        if let Some(cases) = result.get("test_cases") {
            if !cases.is_null() && !cases.is_array() {
                return Err(
                    "validation removal service-result test_cases must be a list or null"
                        .to_string(),
                );
            }
        }
    }
    let present: Vec<&str> = result.keys().map(String::as_str).collect();
    if !schema_fields.iter().all(|field| present.contains(field))
        || !present.iter().all(|field| allowed.contains(field))
    {
        return Err(
            "validation removal service-result fields do not match the producer schema".to_string(),
        );
    }
    if !is_int(result.get("schema_version"))
        || int_value(result.get("schema_version")) != Some(schema)
    {
        return Err(
            "validation removal service-result schema differs from the run-record".to_string(),
        );
    }
    if result.get("commit") != expected_commit {
        return Err(
            "validation removal service-result commit differs from the run-record".to_string(),
        );
    }
    let profile = result.get("profile").and_then(Value::as_str);
    if profile.is_none_or(|text| text.trim().is_empty()) {
        return Err("validation removal service-result profile must be nonempty text".to_string());
    }
    let status = result.get("final_validate_status").and_then(Value::as_str);
    let (result_name, mut expected_exit) = match status {
        Some("PASSED") => ("success", 0),
        Some("FAILED") => ("failure", 1),
        Some("COULD_NOT_RUN") => ("no-result", 75),
        _ => {
            return Err("validation removal service-result final status is unsupported".to_string())
        }
    };
    let mut writeback: Option<Value> = None;
    if schema >= 2 {
        writeback = scorecard_writeback(
            result.get("scorecard_writeback"),
            "validation removal service-result scorecard_writeback",
        )?;
        if status == Some("PASSED")
            && writeback
                .as_ref()
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str)
                == Some("failed")
        {
            expected_exit = 75;
        }
    }
    let exit_code = int_value(result.get("exit_code"));
    if !is_int(result.get("exit_code")) || exit_code != Some(expected_exit) {
        return Err(
            "validation removal service-result exit_code disagrees with its outcome".to_string(),
        );
    }
    let executed_nodes =
        match int_value(result.get("executed_nodes")) {
            Some(number) if number >= 0 && is_int(result.get("executed_nodes")) => number,
            _ => return Err(
                "validation removal service-result executed_nodes must be a non-negative integer"
                    .to_string(),
            ),
        };
    let executed_tests = optional_count(
        result.get("executed_tests"),
        "validation removal service-result executed_tests",
    )?;
    let historical = (1..=3).contains(&schema);
    let mut projection = json_object(&[
        (
            "state",
            Value::String(if historical { "unknown" } else { "completed" }.to_string()),
        ),
        (
            "result",
            Value::String(if historical { "unknown" } else { result_name }.to_string()),
        ),
        (
            "result_source",
            Value::String(
                if historical {
                    "historical-validation-service-result"
                } else {
                    "validation-service-result"
                }
                .to_string(),
            ),
        ),
        (
            "final_validate_status",
            Value::String(status.unwrap_or_default().to_string()),
        ),
        ("exit_code", json!(exit_code.unwrap_or_default())),
        ("executed_nodes", json!(executed_nodes)),
        (
            "executed_tests",
            executed_tests.map_or(Value::Null, |n| json!(n)),
        ),
    ]);
    if historical {
        projection.insert("passed_tests".to_string(), Value::Null);
        projection.insert(
            "detail".to_string(),
            Value::String(historical_detail(schema)),
        );
    }
    if schema >= 2 {
        projection.insert(
            "scorecard_writeback".to_string(),
            writeback.clone().unwrap_or(Value::Null),
        );
    }
    if schema >= 3 {
        let selection = result.get("selection_mode");
        if selection.is_some_and(|value| {
            !value.is_null() && value.as_str().is_none_or(|text| text.trim().is_empty())
        }) {
            return Err(
                "validation removal service-result selection_mode must be nonempty text or null"
                    .to_string(),
            );
        }
        projection.insert(
            "selection_mode".to_string(),
            selection.cloned().unwrap_or(Value::Null),
        );
    }
    if schema >= 4 {
        let passed_tests = optional_count(
            result.get("passed_tests"),
            "validation removal service-result passed_tests",
        )?;
        if executed_tests.is_none() != passed_tests.is_none() {
            return Err("validation removal service-result test counts are incomplete".to_string());
        }
        if let (Some(executed), Some(passed)) = (executed_tests, passed_tests) {
            if passed > executed {
                return Err(
                    "validation removal service-result passed_tests exceeds executed_tests"
                        .to_string(),
                );
            }
        }
        if status == Some("PASSED") && (passed_tests.is_none() || passed_tests != executed_tests) {
            return Err("validation removal service-result PASSED counts disagree".to_string());
        }
        projection.insert(
            "passed_tests".to_string(),
            passed_tests.map_or(Value::Null, |n| json!(n)),
        );
    }
    if schema >= 8 {
        let skipped = optional_count(
            result.get("skipped_tests"),
            "validation removal service-result skipped_tests",
        )?;
        if executed_tests.is_none() != skipped.is_none() {
            return Err("validation removal service-result test counts are incomplete".to_string());
        }
        projection.insert(
            "skipped_tests".to_string(),
            skipped.map_or(Value::Null, |n| json!(n)),
        );
    }
    if schema == 5 {
        match result.get("detail") {
            Some(Value::Array(lines)) => {
                if lines.is_empty()
                    || lines
                        .iter()
                        .any(|line| line.as_str().is_none_or(|text| text.trim().is_empty()))
                    || status != Some("COULD_NOT_RUN")
                {
                    return Err(
                        "validation removal service-result detail violates its schema".to_string(),
                    );
                }
                let joined = lines
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n");
                projection.insert("detail".to_string(), Value::String(joined));
            }
            Some(_) => {
                return Err(
                    "validation removal service-result detail violates its schema".to_string(),
                )
            }
            None => {
                if status == Some("COULD_NOT_RUN") {
                    projection.insert(
                        "detail".to_string(),
                        Value::String(
                            "validation reported COULD_NOT_RUN without detail".to_string(),
                        ),
                    );
                } else {
                    projection.insert("detail".to_string(), Value::Null);
                }
            }
        }
    }
    Ok(projection)
}

/// Python `_frozen_result_evidence_from_obj` (target-checked).
pub(crate) fn frozen_result_evidence(
    value: &Value,
    target: Option<&str>,
) -> Result<(String, u64, u64, u64, String)> {
    let Some(object) = value.as_object() else {
        return Err("durable frozen validation result evidence is incomplete".to_string());
    };
    let keys: Vec<&str> = object.keys().map(String::as_str).collect();
    let sha = object
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let valid = keys.len() == 5
        && ["path", "device", "inode", "size", "sha256"]
            .iter()
            .all(|key| keys.contains(key))
        && target.is_some()
        && sha.len() == 64
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    if !valid {
        return Err("durable frozen validation result evidence is incomplete".to_string());
    }
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "durable frozen validation result evidence is incomplete".to_string())
    };
    let number = |key: &str| {
        int_value(object.get(key))
            .filter(|_| is_int(object.get(key)))
            .ok_or_else(|| "durable frozen validation result evidence is incomplete".to_string())
            .map(|n| n as u64)
    };
    Ok((
        text("path")?,
        number("device")?,
        number("inode")?,
        number("size")?,
        sha.to_string(),
    ))
}

/// Python `_current_incomplete_frozen_result`.
fn current_incomplete_frozen_result(record: &serde_json::Map<String, Value>) -> bool {
    let schema = int_value(record.get("service_result_schema"));
    if !is_int(record.get("service_result_schema")) || !matches!(schema, Some(5..=8)) {
        return false;
    }
    let expected: Vec<(&str, Value)> = vec![
        ("result_source", json!("validation-service-result")),
        ("state", json!("completed")),
        ("result", json!("no-result")),
        ("final_validate_status", json!("COULD_NOT_RUN")),
        ("exit_code", json!(75)),
        ("temporary_checkout", json!(true)),
        ("materialized_target", json!(false)),
        ("qualifying_receipt", json!(false)),
        ("executed_nodes", json!(0)),
        ("executed_tests", Value::Null),
        ("passed_tests", Value::Null),
        ("selection_mode", Value::Null),
        ("scorecard_writeback", Value::Null),
    ];
    expected
        .iter()
        .all(|(key, value)| record.get(*key) == Some(value))
        && !record.contains_key("scorecard_handoff")
        && !record.contains_key("scorecard_writeback_files")
        && has_typed_admission(record)
        && record
            .get("admission_result")
            .and_then(Value::as_object)
            .and_then(|admission| admission.get("state"))
            .and_then(Value::as_str)
            == Some("admitted")
}

/// Python `_current_frozen_service_result`.
fn current_frozen_service_result(record: &serde_json::Map<String, Value>) -> bool {
    let schema = int_value(record.get("service_result_schema"));
    is_int(record.get("service_result_schema"))
        && matches!(schema, Some(4..=8))
        && record.get("result_source").and_then(Value::as_str) == Some("validation-service-result")
        && record.get("state").and_then(Value::as_str) == Some("completed")
        && validation_record_is_terminal(record, "frozen-checkout", false)
}

/// Python `_durable_frozen_result`: the recorded target commit.
fn durable_frozen_result(
    cfg: &Cfg,
    record_path: &Path,
    record: &serde_json::Map<String, Value>,
    target: &str,
) -> Result<String> {
    if current_incomplete_frozen_result(record) {
        return Err(
            "current incomplete frozen validation is retained, never removed by its durable result"
                .to_string(),
        );
    }
    if !current_frozen_service_result(record) {
        return Err("frozen validation has no current service-result terminal result".to_string());
    }
    if !record_process_is_dead(record) {
        return Err("frozen validation process state is live or uncertain".to_string());
    }
    let label = "durable frozen validation result";
    let stem = record_path
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = cfg
        .root
        .join(format!("ignored/validate/frozen/{stem}.jsonl"));
    ensure_no_symlink_components(&cfg.root, &path, label)?;
    if !path.exists() {
        return Err(format!("{label} does not exist: {}", path.display()));
    }
    if record.get("result_record").and_then(Value::as_str) != Some(path.to_string_lossy().as_ref())
    {
        return Err(format!(
            "frozen validation run-record does not name this {label}"
        ));
    }
    let contents = std::fs::read(&path)
        .map_err(|error| format!("cannot read {label} {}: {error}", path.display()))?;
    if contents.len() > 16 * 1024 * 1024 {
        return Err(format!(
            "{label} exceeds the safety bound: {}",
            path.display()
        ));
    }
    let text = String::from_utf8_lossy(&contents);
    let rows: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let Some(last) = rows.last() else {
        return Err(format!("{label} is empty: {}", path.display()));
    };
    let final_row = strict_json_object(last.as_bytes(), &format!("{label} final row"))?;
    if final_row.get("commit").and_then(Value::as_str) != Some(target) {
        return Err(format!(
            "{label} final row does not name the recorded target commit"
        ));
    }
    let checkout = record.get("checkout");
    if checkout.is_none() || final_row.get("cwd") != checkout {
        return Err(format!(
            "{label} final row was not measured in the recorded checkout"
        ));
    }
    Ok(target.to_string())
}

/// The frozen no-proof dispositions (Python `_FrozenNoProofDisposition`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrozenDisposition {
    BlocksEntry,
    CurrentIncompleteRetained,
    CurrentTerminalRemovable,
    HistoricalNonblocking,
    LegacyRecovery,
}

impl FrozenDisposition {
    /// The Python enum's wire value.
    pub(crate) fn wire(self) -> &'static str {
        match self {
            FrozenDisposition::BlocksEntry => "blocks-entry",
            FrozenDisposition::CurrentIncompleteRetained => "current-incomplete-retained",
            FrozenDisposition::CurrentTerminalRemovable => "current-terminal-removable",
            FrozenDisposition::HistoricalNonblocking => "historical-nonblocking",
            FrozenDisposition::LegacyRecovery => "legacy-recovery",
        }
    }
}

/// Python's historical producer-schema location inside the checkout:
/// `_VALIDATION_SERVICE_RESULT_SCHEMA_PATH`.
fn historical_producer_path(checkout: &Path) -> PathBuf {
    checkout.join("ci/manifest-plan/validation-service-result-schema.json")
}

/// Python's historical sidecar location: beside the run-record, named
/// `{record stem}{_VALIDATION_SERVICE_RESULT_SUFFIX}`.
fn historical_sidecar_path(record_path: &Path) -> PathBuf {
    let stem = record_path
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    record_path.with_file_name(format!("{stem}.service-result.json"))
}

/// The historical producer-schema + sidecar projection binding shared
/// by `_frozen_no_proof_disposition` and the historical branch of
/// `_terminal_validation_record`. `expected_commit` is the recorded
/// target commit (the run-record's `target`, not a `commit` field).
fn historical_projection_binding(
    record_path: &Path,
    checkout: &Path,
    record: &serde_json::Map<String, Value>,
    schema: i64,
    expected_commit: &str,
) -> Result<()> {
    let producer_path = historical_producer_path(checkout);
    let sidecar_path = historical_sidecar_path(record_path);
    ensure_no_symlink_components(
        checkout,
        &producer_path,
        "historical frozen validation producer schema",
    )?;
    let producer = std::fs::read(&producer_path).map_err(|error| {
        format!(
            "cannot read historical frozen validation producer schema {}: {error}",
            producer_path.display()
        )
    })?;
    let sidecar = std::fs::read(&sidecar_path).map_err(|error| {
        format!(
            "cannot read historical frozen validation service-result {}: {error}",
            sidecar_path.display()
        )
    })?;
    let fields = service_result_schema(&producer, schema)?;
    let expected = Value::String(expected_commit.to_string());
    let projection = service_result_projection(&sidecar, schema, &fields, Some(&expected))?;
    for (field, expected_value) in &projection {
        if record.get(field) != Some(expected_value) {
            return Err(format!(
                "historical frozen validation service-result {field} differs from the run-record"
            ));
        }
    }
    if projection.get("scorecard_writeback") != Some(&json!({"status": "completed"})) {
        return Err(
            "historical frozen validation has no completed scorecard writeback".to_string(),
        );
    }
    Ok(())
}

/// Python `_frozen_no_proof_disposition`; the census is injected.
/// Returns (disposition, historical schema when historical).
pub(crate) fn frozen_no_proof_disposition(
    cfg: &Cfg,
    record_path: &Path,
    checkout: &Path,
    repository: &Path,
    record_digest: &str,
    record_target: &str,
    census_unused: &dyn Fn(&Path) -> Result<()>,
) -> Result<(FrozenDisposition, Option<i64>)> {
    let contents = std::fs::read(record_path).map_err(|error| {
        format!(
            "cannot read frozen validation run-record {}: {error}",
            record_path.display()
        )
    })?;
    let record = strict_json_object(&contents, "frozen validation run-record")?;
    let schema = int_value(record.get("service_result_schema"));
    let source = record.get("result_source").and_then(Value::as_str);
    if record.contains_key("result_source")
        && source != Some("validation-service-result")
        && !(matches!(schema, Some(1..=3))
            && source == Some("historical-validation-service-result"))
    {
        return Ok((FrozenDisposition::LegacyRecovery, None));
    }
    if matches!(schema, Some(4..=8)) && source == Some("validation-service-result") {
        if current_incomplete_frozen_result(&record) {
            return Ok((FrozenDisposition::CurrentIncompleteRetained, None));
        }
        if current_frozen_service_result(&record) {
            if !record_process_is_dead(&record) {
                return Ok((FrozenDisposition::BlocksEntry, None));
            }
            durable_frozen_result(cfg, record_path, &record, record_target)?;
            return Ok((FrozenDisposition::CurrentTerminalRemovable, None));
        }
        return Ok((FrozenDisposition::BlocksEntry, None));
    }
    // Historical schemas: producer-schema + sidecar projection.
    if matches!(schema, Some(2..=3))
        && record.get("result_source").and_then(Value::as_str)
            == Some("historical-validation-service-result")
    {
        let schema = schema.unwrap_or_default();
        if record.get("kind").and_then(Value::as_str) != Some("frozen-validate")
            || record.get("materialized_target") != Some(&Value::Bool(false))
            || record.contains_key("scorecard_handoff")
            || record.contains_key("scorecard_writeback_files")
        {
            return Err(
                "historical frozen validation without a removal proof does not match the pre-proof non-materialized shape"
                    .to_string(),
            );
        }
        if !record_process_is_dead(&record) {
            return Err(
                "historical frozen validation process state is live or uncertain".to_string(),
            );
        }
        historical_projection_binding(record_path, checkout, &record, schema, record_target)?;
        census_unused(checkout)?;
        // Re-read and re-bind before deciding.
        let rebound = std::fs::read(record_path).map_err(|error| {
            format!(
                "cannot read frozen validation run-record {}: {error}",
                record_path.display()
            )
        })?;
        if rebound != contents {
            return Err(
                "historical frozen validation evidence changed during entry classification"
                    .to_string(),
            );
        }
        let _ = (cfg, repository, record_digest);
        let head = git_text(checkout, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        if head != record_target {
            return Err(
                "historical frozen validation checkout HEAD changed during entry classification"
                    .to_string(),
            );
        }
        return Ok((FrozenDisposition::HistoricalNonblocking, Some(schema)));
    }
    Ok((FrozenDisposition::BlocksEntry, None))
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
        return terminal_frozen_record(cfg, &path, record, target);
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

/// The frozen branch of `_terminal_validation_record`:
/// (relative, sha256, recorded target commit).
fn terminal_frozen_record(
    cfg: &Cfg,
    path: &Path,
    record: &serde_json::Map<String, Value>,
    target: &Path,
) -> Result<(String, String, Option<String>)> {
    let relative = path
        .strip_prefix(&cfg.root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());
    let contents = std::fs::read(path).unwrap_or_default();
    let digest = format!("{:x}", Sha256::digest(&contents));
    let recorded_checkout = record.get("checkout").and_then(Value::as_str);
    if recorded_checkout != Some(target.to_string_lossy().as_ref()) {
        return Err("terminal validation record does not name this exact checkout".to_string());
    }
    if record.get("temporary_checkout") == Some(&Value::Bool(false)) {
        return Err("terminal validation record says the checkout is not temporary".to_string());
    }
    if record
        .get("admission_result")
        .and_then(Value::as_object)
        .and_then(|admission| admission.get("state"))
        .and_then(Value::as_str)
        == Some("refused")
    {
        return Err(
            "legacy frozen validation record records a refused admission and is not removal evidence"
                .to_string(),
        );
    }
    let schema = int_value(record.get("service_result_schema"));
    let source = record.get("result_source").and_then(Value::as_str);
    // Current schemas: durable-result authority.
    if matches!(schema, Some(4..=8)) && source == Some("validation-service-result") {
        if current_incomplete_frozen_result(record) {
            return Err(
                "current incomplete frozen validation is retained, never removed".to_string(),
            );
        }
        let target_commit = record
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        durable_frozen_result(cfg, path, record, &target_commit)?;
        return Ok((relative, digest, Some(target_commit)));
    }
    // Historical schemas: producer-schema projection binding.
    if matches!(schema, Some(1..=3)) && is_int(record.get("service_result_schema")) {
        let schema = schema.unwrap_or_default();
        let target_commit = record
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        historical_projection_binding(path, target, record, schema, &target_commit)?;
        return Ok((relative, digest, Some(target_commit)));
    }
    Err(
        "direct frozen validation recovery requires --validation-proof-manifest for current or historical-projection records; use the bounded batch command to classify a pre-proof historical checkout without removing it"
            .to_string(),
    )
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

    const DISPOSITION_DRIVER: &str = r####"import json, sys
sys.path.insert(0, "py")
from pathlib import Path
from wrkslots import cli

root = Path(sys.argv[1])
config = cli._load_config(str(root), "node-a")
checkout = Path(sys.argv[2])
repository = Path(sys.argv[3])
record_path = Path(sys.argv[4])
target = sys.argv[5]
out = {}
record = json.loads(record_path.read_text())
census = cli._ProcessPathCensus((), (), owner_cgroup_complete=False)
budget = cli._ReadOnlyCommandBudget.start(
    timeout_seconds=600.0,
    stdout_limit=16 * 1024 * 1024,
    stderr_limit=16 * 1024 * 1024,
    input_limit=16 * 1024 * 1024,
)
identity = cli._private_cleanup_path_identity(
    config, checkout, allowed_modes={0o700}, managed_root=None
)
batch = cli._OwnerlessValidationBatchContext(
    {identity: checkout}, census, budget
)

def canonical(_config, _path, contents):
    parsed = cli._strict_json_object(contents, "frozen fixture")
    return (
        {
            "schema_version": parsed["schema_version"],
            "kind": parsed["kind"],
            "state": parsed["state"],
            "target": parsed["target"],
            "repo": parsed["repo"],
        },
        parsed,
    )

cli._canonical_frozen_validation_record = canonical
# The batch fresh-census is injected evidence on the Rust side too
# (assert_slot_unused on an unused clone); stub it as Python's own
# lifecycle fixtures do, so host process churn cannot close it.
cli._capture_same_uid_process_path_census = (
    lambda _paths, budget=None: cli._ProcessPathCensus((), ())
)
raw_record = "ignored/validate/runs/run-frozen.json"
try:
    classification = cli._frozen_no_proof_disposition(
        config,
        checkout,
        raw_record,
        "upstream-repo",
        batch_cleanup=batch,
    )
    out["ok"] = classification.disposition.value
except Exception as exc:
    out["err"] = str(exc)
print(json.dumps(out, sort_keys=True))
"####;

    fn test_schema() -> i64 {
        2
    }

    fn test_stride() -> i64 {
        2
    }

    #[test]
    fn frozen_disposition_historical_schema2_matches_python() {
        let (cfg, root) = fixture("frozen-disposition");
        // Build the run-1773 shape Python's own lifecycle fixtures use:
        // the producer schema is committed in the source repository, the
        // checkout is a standalone clone detached at the recorded
        // target, and the service-result sidecar sits beside the record.
        let upstream = root.join("upstream-repo");
        let producer = json!({
            "schema_version": test_schema(),
            "fields": [
                "schema_version", "commit", "profile", "final_validate_status",
                "exit_code", "executed_nodes", "executed_tests", "scorecard_writeback"
            ],
            "outcomes": {
                "PASSED": {"validation_exit_code": 0, "state": "completed", "result": "success"},
                "FAILED": {"validation_exit_code": 1, "state": "completed", "result": "failure"},
                "COULD_NOT_RUN": {"validation_exit_code": 75, "state": "completed", "result": "no-result"}
            },
            "scorecard_writeback": {"nullable": true, "variants": {"completed": ["status"], "failed": ["status", "error"]}}
        });
        let schema_path = upstream.join("ci/manifest-plan/validation-service-result-schema.json");
        std::fs::create_dir_all(schema_path.parent().expect("parent")).expect("dirs");
        std::fs::write(&schema_path, producer.to_string()).expect("producer");
        std::fs::write(upstream.join("file.txt"), "one\n").expect("file");
        git_text(&upstream, &["init", "-q", "-b", "main"]).expect("init");
        git_text(&upstream, &["add", "."]).expect("add");
        git_text(
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
        )
        .expect("commit");
        git_text(
            &upstream,
            &[
                "remote",
                "add",
                "origin",
                upstream.to_string_lossy().as_ref(),
            ],
        )
        .expect("remote");
        git_text(
            &upstream,
            &[
                "fetch",
                "-q",
                "origin",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        )
        .expect("fetch");
        let record_target = git_text(&upstream, &["rev-parse", "HEAD"])
            .expect("head")
            .trim()
            .to_owned();
        let checkout = root.join("worktrees/validate/val-a");
        std::fs::create_dir_all(checkout.parent().expect("parent")).expect("dirs");
        git_text(
            &root,
            &[
                "clone",
                "-q",
                "--no-checkout",
                "--no-tags",
                upstream.to_string_lossy().as_ref(),
                checkout.to_string_lossy().as_ref(),
            ],
        )
        .expect("clone");
        git_text(&checkout, &["checkout", "-q", "--detach", &record_target]).expect("detach");
        std::fs::set_permissions(
            &checkout,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .expect("checkout mode");
        let sidecar = json!({
            "schema_version": test_schema(),
            "commit": record_target,
            "profile": "only-portable",
            "final_validate_status": "FAILED",
            "exit_code": 1,
            "executed_nodes": 14,
            "executed_tests": null,
            "scorecard_writeback": {"status": "completed"}
        });
        std::fs::write(
            root.join("ignored/validate/runs/run-frozen.service-result.json"),
            sidecar.to_string(),
        )
        .expect("sidecar");
        let record = serde_json::json!({
            "schema_version": 1,
            "kind": "frozen-validate",
            "producer": "ci-hub/validate/run_registry.py",
            "admission": "frozen-validate",
            "validation_kind": "frozen-validate",
            "service_result_schema": test_stride(),
            "result_source": "historical-validation-service-result",
            "materialized_target": false,
            "qualifying_receipt": false,
            "state": "unknown",
            "result": "unknown",
            "final_validate_status": "FAILED",
            "exit_code": 1,
            "executed_nodes": 14,
            "executed_tests": null,
            "passed_tests": null,
            "detail": "validation-service-result-schema_version: historical schema 2 is readable but has no current authority",
            "scorecard_writeback": {"status": "completed"},
            "process_identity": {"pid": 999999, "start_ticks": 1, "boot_id": "historical-boot"},
            "checkout": checkout.to_string_lossy(),
            "source_checkout": upstream.to_string_lossy(),
            "target": record_target,
            "repo": "consumer-a/project",
            "temporary_checkout": true
        });
        let record_path = root.join("ignored/validate/runs/run-frozen.json");
        std::fs::write(&record_path, record.to_string()).expect("record");

        let mine = frozen_no_proof_disposition(
            &cfg,
            &record_path,
            &checkout,
            &root.join("upstream-repo"),
            &"0".repeat(64),
            &record_target,
            &|path| crate::remove_write::assert_slot_unused(path, i64::from(std::process::id())),
        );
        let driver = std::env::temp_dir().join(format!(
            "wrkslotsd-validation-disposition-driver-{}.py",
            std::process::id()
        ));
        std::fs::write(&driver, DISPOSITION_DRIVER).expect("driver");
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
            .arg(&record_path)
            .arg(&record_target)
            .current_dir(&repo_root)
            .output()
            .expect("python oracle");
        assert!(
            output.status.success(),
            "oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected: Value = serde_json::from_slice(&output.stdout).expect("oracle json");
        match (&mine, expected.get("ok").and_then(Value::as_str)) {
            (Ok((disp, _)), Some(value)) => {
                let mine_value = match disp {
                    FrozenDisposition::HistoricalNonblocking => "historical-nonblocking",
                    FrozenDisposition::CurrentTerminalRemovable => "current-terminal-removable",
                    FrozenDisposition::BlocksEntry => "blocks-entry",
                    FrozenDisposition::CurrentIncompleteRetained => "current-incomplete-retained",
                    FrozenDisposition::LegacyRecovery => "legacy-recovery",
                };
                assert_eq!(mine_value, value);
            }
            (Err(error), None) => {
                assert_eq!(
                    expected.get("err").and_then(Value::as_str),
                    Some(error.as_str())
                );
            }
            other => panic!("mismatch: {other:?} vs oracle {expected}"),
        }
        // Negative: dropping materialized_target breaks the pre-proof
        // shape, and both editions refuse with the same line.
        let mut broken: Value = serde_json::from_str(&record.to_string()).expect("record json");
        broken
            .as_object_mut()
            .expect("record object")
            .remove("materialized_target");
        std::fs::write(&record_path, broken.to_string()).expect("broken record");
        let mine = frozen_no_proof_disposition(
            &cfg,
            &record_path,
            &checkout,
            &root.join("upstream-repo"),
            &"0".repeat(64),
            &record_target,
            &|path| crate::remove_write::assert_slot_unused(path, i64::from(std::process::id())),
        );
        let output = std::process::Command::new("python3")
            .arg(&driver)
            .arg(&root)
            .arg(&checkout)
            .arg(root.join("upstream-repo"))
            .arg(&record_path)
            .arg(&record_target)
            .current_dir(&repo_root)
            .output()
            .expect("python oracle");
        assert!(
            output.status.success(),
            "oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected: Value = serde_json::from_slice(&output.stdout).expect("oracle json");
        let Err(error) = mine else {
            panic!("expected shape refusal, got {mine:?} vs oracle {expected}");
        };
        assert_eq!(
            expected.get("err").and_then(Value::as_str),
            Some(error.as_str())
        );
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
