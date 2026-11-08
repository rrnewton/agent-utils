//! Writer: Python-shaped `create` output (`create-text`).
//!
//! Runs the same fixture-guarded create mutation as `create`, then
//! reproduces the Python edition's post-registration setup-hook
//! installers (`scripts/setup-hooks.sh` in repositories private to
//! the new slot) and its human/JSON rendering, byte for byte.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::active_state::load_cfg;
use crate::finish_write::stored_path;

type Result<T> = std::result::Result<T, String>;

const SCRIPT: &str = "scripts/setup-hooks.sh";
const TIMEOUT_SECONDS: f64 = 60.0;
const CLEARED_ENV: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let (code, stdout, stderr) = git_check(repo, args)?;
    if code != 0 {
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(stdout.trim().to_string())
}

fn git_check(repo: &Path, args: &[&str]) -> Result<(i32, String, String)> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-replace-objects",
            "-c",
            "core.useReplaceRefs=false",
            "-C",
        ])
        .arg(repo)
        .args(args)
        .env_clear();
    for (key, value) in std::env::vars() {
        if !key.starts_with("GIT_") {
            command.env(key, value);
        }
    }
    command
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    let output = command
        .output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

fn git_dirs(repository: &Path) -> Result<(PathBuf, PathBuf)> {
    let out = git(
        repository,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ],
    )?;
    let lines: Vec<&str> = out.lines().collect();
    if lines.len() != 2 || lines.iter().any(|line| line.is_empty()) {
        return Err(format!(
            "git rev-parse in {} printed {out:?}",
            repository.display()
        ));
    }
    let resolve = |raw: &str| -> Result<PathBuf> {
        let path = PathBuf::from(raw);
        let absolute = if path.is_absolute() {
            path
        } else {
            repository.join(path)
        };
        Ok(absolute.canonicalize().unwrap_or(absolute))
    };
    Ok((resolve(lines[0])?, resolve(lines[1])?))
}

fn indexed_gitlinks(repository: &Path) -> Result<Vec<String>> {
    let (_code, stdout, _stderr) = git_check(repository, &["ls-files", "--stage", "-z"])?;
    let mut paths: Vec<String> = Vec::new();
    for record in stdout.split('\0') {
        if record.is_empty() {
            continue;
        }
        if let Some((metadata, path)) = record.split_once('\t') {
            if metadata.starts_with("160000 ") && !paths.iter().any(|item| item == path) {
                paths.push(path.to_string());
            }
        }
    }
    Ok(paths)
}

pub(crate) fn submodule_populated(nested: &Path) -> bool {
    let entry = nested.join(".git");
    if entry.is_dir() {
        return true;
    }
    if !entry.is_file() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(&entry) else {
        return false;
    };
    let Some(target) = text.trim().strip_prefix("gitdir:") else {
        return false;
    };
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    let path = PathBuf::from(target);
    let resolved = if path.is_absolute() {
        path
    } else {
        nested.join(path)
    };
    resolved.is_dir()
}

fn setup_repositories(checkout: &Path) -> Result<Vec<PathBuf>> {
    fn visit(repository: &Path, prefix: &str, found: &mut Vec<String>) -> Result<()> {
        for relative in indexed_gitlinks(repository)? {
            let nested = repository.join(&relative);
            if nested
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
            {
                return Err(format!(
                    "nested repository path is unsafe: {}",
                    nested.display()
                ));
            }
            if !submodule_populated(&nested) {
                continue;
            }
            let display = format!("{prefix}{relative}");
            found.push(display.clone());
            visit(&nested, &format!("{display}/"), found)?;
        }
        Ok(())
    }
    let mut found: Vec<String> = Vec::new();
    visit(checkout, "", &mut found)?;
    for path in &found {
        let candidate = Path::new(path);
        if candidate.is_absolute() || candidate.components().any(|c| c.as_os_str() == "..") {
            return Err(format!(
                "Git reported an unsafe initialized submodule path: {path:?}"
            ));
        }
    }
    let mut repositories = vec![checkout.to_path_buf()];
    let mut seen: Vec<String> = Vec::new();
    for path in found {
        if !seen.contains(&path) {
            seen.push(path.clone());
            repositories.push(checkout.join(path));
        }
    }
    Ok(repositories)
}

#[derive(Clone)]
struct HookResult {
    path: String,
    status: String,
    returncode: Option<i64>,
    output_tail: String,
    detail: String,
}

impl HookResult {
    fn new(path: String, status: &str) -> Self {
        Self {
            path,
            status: status.to_string(),
            returncode: None,
            output_tail: String::new(),
            detail: String::new(),
        }
    }
    fn to_obj(&self) -> Value {
        json!({
            "path": self.path,
            "script": SCRIPT,
            "status": self.status,
            "returncode": self.returncode,
            "output_tail": self.output_tail,
            "detail": self.detail,
        })
    }
}

fn output_tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(4096);
    let text = String::from_utf8_lossy(&bytes[start..]).into_owned();
    let mut lines: Vec<&str> = text.lines().collect();
    if bytes.len() > 4096 && !lines.is_empty() {
        lines.remove(0);
    }
    let keep = lines.len().saturating_sub(20);
    lines[keep..].join("\n")
}

fn run_one_installer(repository: &Path) -> HookResult {
    let script = repository.join(SCRIPT);
    let mut temp_path = std::env::temp_dir();
    temp_path.push(format!(
        "wrkslotsd-setup-hooks-{}-{}",
        std::process::id(),
        repository
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>()
    ));
    let file = match std::fs::File::create(&temp_path) {
        Ok(file) => file,
        Err(error) => {
            let mut result =
                HookResult::new(repository.to_string_lossy().into_owned(), "failed-to-start");
            result.detail = format!("cannot create its output file: {error}");
            return result;
        }
    };
    let stdout = match file.try_clone() {
        Ok(clone) => clone,
        Err(error) => {
            let mut result =
                HookResult::new(repository.to_string_lossy().into_owned(), "failed-to-start");
            result.detail = format!("cannot create its output file: {error}");
            return result;
        }
    };
    let mut command = Command::new(&script);
    command
        .current_dir(repository)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::from(file))
        .env_clear();
    for (key, value) in std::env::vars() {
        if !CLEARED_ENV.contains(&key.as_str()) {
            command.env(key, value);
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = std::fs::remove_file(&temp_path);
            let mut result =
                HookResult::new(repository.to_string_lossy().into_owned(), "failed-to-start");
            result.detail = error.to_string();
            return result;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break None,
        }
    };
    let mut bytes: Vec<u8> = Vec::new();
    if let Ok(mut read) = std::fs::File::open(&temp_path) {
        let _ = read.read_to_end(&mut bytes);
    }
    let _ = std::fs::remove_file(&temp_path);
    let tail = output_tail(&bytes);
    let mut result = HookResult::new(repository.to_string_lossy().into_owned(), "failed");
    result.output_tail = tail;
    match status {
        Some(exit) => {
            let code = exit.code().unwrap_or(-1);
            result.returncode = Some(i64::from(code));
            result.status = if code == 0 { "ran" } else { "failed" }.to_string();
        }
        None => {
            result.status = "timed-out".to_string();
            result.returncode = Some(-1);
            result.detail = "stopped after 60 s".to_string();
        }
    }
    result
}

fn run_setup_hook_installers(
    cfg: &crate::active_state::Cfg,
    checkouts: &[Value],
) -> (i64, Vec<HookResult>) {
    let mut checked: i64 = 0;
    let mut results: Vec<HookResult> = Vec::new();
    for checkout in checkouts {
        let raw_path = checkout
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let checkout_path = match stored_path(cfg, raw_path, "checkout path") {
            Ok(path) => path,
            Err(error) => {
                let mut result = HookResult::new(raw_path.to_string(), "inspection-failed");
                result.detail = error;
                results.push(result);
                continue;
            }
        };
        let (slot_git_dir, repositories) = match git_dirs(&checkout_path)
            .and_then(|(git_dir, _)| Ok((git_dir, setup_repositories(&checkout_path)?)))
        {
            Ok(pair) => pair,
            Err(error) => {
                let mut result = HookResult::new(
                    checkout_path.to_string_lossy().into_owned(),
                    "inspection-failed",
                );
                result.detail = error;
                results.push(result);
                continue;
            }
        };
        for repository in &repositories {
            checked += 1;
            let script = repository.join(SCRIPT);
            let metadata = match script.symlink_metadata() {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    let mut result = HookResult::new(
                        repository.to_string_lossy().into_owned(),
                        "inspection-failed",
                    );
                    result.detail = error.to_string();
                    results.push(result);
                    continue;
                }
            };
            {
                use std::os::unix::fs::PermissionsExt as _;
                let mode = metadata.permissions().mode();
                if !metadata.is_file() || mode & 0o100 == 0 {
                    let mut result = HookResult::new(
                        repository.to_string_lossy().into_owned(),
                        "not-executable",
                    );
                    result.detail = format!("{SCRIPT} is not a regular executable file");
                    results.push(result);
                    continue;
                }
            }
            let common_dir = match git_dirs(repository) {
                Ok((_, common)) => common,
                Err(error) => {
                    let mut result = HookResult::new(
                        repository.to_string_lossy().into_owned(),
                        "inspection-failed",
                    );
                    result.detail = error;
                    results.push(result);
                    continue;
                }
            };
            if !common_dir.starts_with(&slot_git_dir) {
                let mut result = HookResult::new(
                    repository.to_string_lossy().into_owned(),
                    "not-run-shared-git-config",
                );
                result.detail = format!(
                    "its Git configuration {} is shared beyond this slot",
                    common_dir.display()
                );
                results.push(result);
                continue;
            }
            let (code, stdout, stderr) = match git_check(
                repository,
                &["config", "--local", "--get", "core.hooksPath"],
            ) {
                Ok(triple) => triple,
                Err(error) => {
                    let mut result = HookResult::new(
                        repository.to_string_lossy().into_owned(),
                        "inspection-failed",
                    );
                    result.detail = error;
                    results.push(result);
                    continue;
                }
            };
            if code == 1 {
                // No configured hooksPath: run the installer below.
            } else if code != 0 {
                let mut result = HookResult::new(
                    repository.to_string_lossy().into_owned(),
                    "inspection-failed",
                );
                result.detail = format!(
                    "git config --local --get core.hooksPath in {} exited {}: {}",
                    repository.display(),
                    code,
                    stderr.trim()
                );
                results.push(result);
                continue;
            } else {
                let mut result = HookResult::new(
                    repository.to_string_lossy().into_owned(),
                    "not-run-hooks-already-configured",
                );
                result.detail = format!("core.hooksPath is already {}", stdout.trim());
                results.push(result);
                continue;
            }
            results.push(run_one_installer(repository));
        }
    }
    (checked, results)
}

fn hook_lines(checked: i64, results: &[HookResult]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for result in results {
        let where_ = format!("{SCRIPT} in {}", result.path);
        match result.status.as_str() {
            "ran" => lines.push(format!("hooks: ran {where_} (rc 0)")),
            "not-run-shared-git-config" | "not-run-hooks-already-configured" => {
                lines.push(format!("hooks: not run: {where_}; {}", result.detail))
            }
            other => {
                let outcome = match other {
                    "failed" => format!("failed (rc {})", result.returncode.unwrap_or(-1)),
                    "timed-out" => format!("timed out ({})", result.detail),
                    "failed-to-start" => format!("could not start: {}", result.detail),
                    "not-executable" => format!("not run: {}", result.detail),
                    _ => format!("not inspected: {}", result.detail),
                };
                lines.push(format!(
                    "hooks: WARNING {where_} {outcome}; the slot was created without these Git hooks"
                ));
                if !result.output_tail.is_empty() {
                    lines.push("hooks: output tail:".to_string());
                    for line in result.output_tail.lines() {
                        lines.push(format!("hooks: | {line}"));
                    }
                }
            }
        }
    }
    if results
        .iter()
        .all(|result| result.status == "not-run-shared-git-config")
    {
        lines.push(format!(
            "hooks: skipped; no repository private to this slot has an executable {SCRIPT} ({checked} checked)"
        ));
    }
    lines
}

fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let mut sorted = serde_json::Map::new();
            for (key, item) in entries {
                sorted.insert(key.clone(), sort_keys(item));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// Create a slot and print the Python `create` output: the human
/// lines, or the JSON payload with the hook report on stderr.
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_text(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    slot_type: &str,
    coordinator_authorized: bool,
    agent: &str,
    task: &str,
    purpose: &str,
    owner_pid: Option<i64>,
    coordinator_pid: i64,
    repos: &[String],
    remotes_in: &[String],
    remote_urls: &[String],
    branches_in: &[String],
    starts_in: &[String],
    override_disk_floor: bool,
    representation: &str,
    task_scope_unit: Option<&str>,
    task_scope_invocation_id: Option<&str>,
    format_json: bool,
) -> Result<String> {
    let value = crate::create_write::create_slot_text(
        root,
        machine_override,
        slot,
        slot_type,
        coordinator_authorized,
        agent,
        task,
        purpose,
        owner_pid,
        coordinator_pid,
        repos,
        remotes_in,
        remote_urls,
        branches_in,
        starts_in,
        override_disk_floor,
        representation,
        task_scope_unit,
        task_scope_invocation_id,
    )?;
    let (cfg, _selected) = load_cfg(root, machine_override)?;
    let checkouts = value
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let (checked, results) = run_setup_hook_installers(&cfg, &checkouts);
    let lines = hook_lines(checked, &results);
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let retained = value
        .get("retained_storage_inconsistencies")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let retained_count = retained.as_array().map(Vec::len).unwrap_or(0);
    if format_json {
        for line in &lines {
            eprintln!("{line}");
        }
        let mut checkout_objs: Vec<Value> = Vec::new();
        for checkout in &checkouts {
            let raw_path = checkout
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let absolute = stored_path(&cfg, raw_path, "checkout path")?;
            checkout_objs.push(json!({
                "name": checkout.get("name").and_then(Value::as_str).unwrap_or_default(),
                "path": absolute.to_string_lossy().into_owned(),
                "head": checkout.get("head").and_then(Value::as_str).unwrap_or_default(),
            }));
        }
        let payload = json!({
            "slot": text("slot"),
            "slot_type": text("slot_type"),
            "setup_hooks": {
                "script": SCRIPT,
                "timeout_seconds": TIMEOUT_SECONDS,
                "repositories_checked": checked,
                "results": results.iter().map(HookResult::to_obj).collect::<Vec<_>>(),
            },
            "agent": text("agent"),
            "generation": value.get("generation").and_then(Value::as_i64).unwrap_or(1),
            "owner_process": text("owner_process"),
            "retained_storage_inconsistencies": retained,
            "checkouts": checkout_objs,
        });
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&sort_keys(&payload)).unwrap_or_default()
        ));
    }
    let mut out = format!(
        "created slot={} agent={} generation={} checkouts={} retained_storage_inconsistencies={}\n",
        text("slot"),
        text("agent"),
        value.get("generation").and_then(Value::as_i64).unwrap_or(1),
        checkouts.len(),
        retained_count
    );
    for checkout in &checkouts {
        let raw_path = checkout
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let absolute = stored_path(&cfg, raw_path, "checkout path")?;
        out.push_str(&format!(
            "checkout={} path={} head={}\n",
            checkout
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            absolute.display(),
            checkout
                .get("head")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ));
    }
    if text("owner_process") == "unbound" {
        out.push_str("owner_process=unbound; run 'wrkslots adopt' before heartbeat or finish\n");
    }
    for line in &lines {
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}
