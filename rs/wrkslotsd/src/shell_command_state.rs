//! Python `shell-command` parity: print the exec-only line that enters
//! a slot's box (or a coordinator box).
//!
//! Mirrors `_cmd_shell_command`: validate the box name or find the
//! active record, require the systemd user D-Bus (the boxed shell must
//! keep the pane's process ID), then print one quoted line, or the
//! `{command, slot_path, isolation}` JSON envelope with `--format
//! json`. The printed line still names the Python `run`/`box` entry
//! (`python3 -I py/wrkslots/__main__.py`), exactly as the Python
//! edition prints it: `run` and `box` are not ported yet, so the
//! boxed step stays Python in both editions until they are. The
//! Python interpreter is `python3` resolved on PATH (the differential
//! drives the Python edition as `python3 -m wrkslots`, whose
//! `sys.executable` is that same resolved interpreter), and the
//! `__main__.py` path is located above this executable, in the
//! checkout the binary was built from.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::active_state::{load_cfg, parse_state_ordered, slot_directory};
use crate::global_state::{event_log_status, EventLogStatus};

type Result<T> = std::result::Result<T, String>;

const ISOLATIONS: [&str; 3] = ["userns", "cgroup", "root"];
const COORDINATOR_SCOPES: [&str; 2] = ["worktrees", "project"];

fn name_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn validate_name(value: &str, label: &str) -> Result<()> {
    if name_valid(value) {
        Ok(())
    } else {
        Err(format!(
            "invalid {label} '{value}'; use 1-64 letters, digits, '.', '_', or '-'"
        ))
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// `shutil.which`: search PATH (a name containing a slash is used as-is
/// and must itself be executable).
fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return is_executable(&path).then_some(path);
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(program);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// `os.path.realpath` for the paths this command prints: resolve every
/// symlink when the path exists; otherwise the lexical absolute form.
fn realpath(path: &Path) -> String {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved.to_string_lossy().into_owned();
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    crate::storage_state::py_normpath(&absolute.to_string_lossy())
}

/// Whether a process can join a transient scope without forking (the
/// D-Bus path), as `sandbox.scope_in_place_available` probes it.
fn scope_in_place_available() -> bool {
    let Some(busctl) = which("busctl") else {
        return false;
    };
    Command::new(busctl)
        .args(["--user", "status", "org.freedesktop.systemd1"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// The interpreter the printed line names: `os.path.realpath(
/// sys.executable)` of a `python3` process. Resolving the PATH entry
/// is not enough on hosts where `python3` is a launcher shim that
/// execs a versioned binary (the Python edition names the versioned
/// binary, a wrapper that forked would change the pane's PID), so ask
/// the interpreter itself, once.
fn python_interpreter() -> Result<String> {
    let Some(python) = which("python3") else {
        return Err(
            "shell-command needs python3 on PATH to name the boxed run interpreter".to_owned(),
        );
    };
    let output = Command::new(&python)
        .args([
            "-c",
            "import os, sys; print(os.path.realpath(sys.executable))",
        ])
        .output()
        .map_err(|error| format!("cannot run python3 to name its interpreter: {error}"))?;
    if !output.status.success() {
        return Err("cannot run python3 to name its interpreter".to_owned());
    }
    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if resolved.is_empty() {
        return Err("python3 named no interpreter".to_owned());
    }
    Ok(resolved)
}

fn python_main() -> Result<String> {
    let exe =
        std::env::current_exe().map_err(|error| format!("cannot locate executable: {error}"))?;
    for ancestor in exe.ancestors() {
        let candidate = ancestor.join("py/wrkslots/__main__.py");
        if candidate.is_file() {
            return Ok(realpath(&candidate));
        }
    }
    Err(format!(
        "cannot locate py/wrkslots/__main__.py above {}",
        exe.display()
    ))
}

fn active_slots(
    root: Option<&Path>,
    cfg: &crate::active_state::Cfg,
    selected: &str,
) -> Result<Vec<Value>> {
    let events_dir = cfg.control.join(format!("EVENTS.{selected}"));
    let document = if matches!(event_log_status(&events_dir), EventLogStatus::Derived) {
        crate::hold_write::active_document(root, cfg, selected)?
    } else {
        parse_state_ordered(root, Some(selected))?.0
    };
    Ok(document
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// The Python `shell-command` output: one exec line, or its JSON form.
///
/// `command` is the boxed program after the literal `--`; empty means
/// the interactive shell. Validation order mirrors Python: box name or
/// active-record refusal first, then the D-Bus refusal.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shell_command_cli(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: Option<&str>,
    slot_type: &str,
    box_mode: bool,
    name: &str,
    writable: Option<&str>,
    cwd: Option<&Path>,
    isolation: Option<&str>,
    shell: Option<&str>,
    format_json: bool,
    command: &[String],
    after_separator: &[String],
) -> Result<String> {
    if !ISOLATIONS.contains(&isolation.unwrap_or("userns")) && isolation.is_some() {
        return Err(format!(
            "isolation must be one of {}",
            ISOLATIONS.join(", ")
        ));
    }
    if let Some(scope) = writable {
        if !COORDINATOR_SCOPES.contains(&scope) {
            return Err(format!(
                "writable scope must be one of {}",
                COORDINATOR_SCOPES.join(", ")
            ));
        }
    }
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let mut effective_command: Vec<String> = command.to_vec();
    let box_cwd: Option<PathBuf> = if box_mode {
        if let Some(slot) = slot {
            // With --box there is no SLOT: the parser hands the first
            // word after `--` to it, as Python's argparse does. A word
            // before `--` is a mistaken SLOT, not a boxed program.
            if after_separator.first().map(String::as_str) != Some(slot) {
                return Err(format!(
                    "shell-command --box takes no SLOT ('{slot}'); put a program to run boxed after a literal --"
                ));
            }
            effective_command = std::iter::once(slot.to_owned())
                .chain(effective_command)
                .collect();
        }
        validate_name(name, "box name")?;
        let cwd = match cwd {
            Some(given) if given.is_absolute() => given.to_path_buf(),
            Some(given) => std::env::current_dir()
                .map_err(|error| format!("cannot resolve box cwd: {error}"))?
                .join(given),
            None => cfg.root.clone(),
        };
        Some(cwd)
    } else {
        let Some(slot) = slot else {
            return Err("shell-command needs a SLOT, or --box for a coordinator box".to_owned());
        };
        let slots = active_slots(root, &cfg, &selected)?;
        // The Python loader resolves every record's source repository
        // while loading (its absence refuses before the slot lookup).
        for record in &slots {
            if let Some(checkouts) = record.get("checkouts").and_then(Value::as_array) {
                for checkout in checkouts {
                    if let Some(repository) = checkout.get("repository").and_then(Value::as_str) {
                        crate::finish_write::stored_repository_path(&cfg, repository)?;
                    }
                }
            }
        }
        if !slots
            .iter()
            .any(|record| record.get("slot").and_then(Value::as_str) == Some(slot))
        {
            return Err(format!("slot '{slot}' is not active on machine {selected}"));
        }
        None
    };
    if !scope_in_place_available() {
        return Err(
            "shell-command needs the systemd user manager's D-Bus API (busctl --user) so the boxed shell keeps the pane's process ID; it is unavailable here"
                .to_owned(),
        );
    }
    let shell_name = shell
        .map(str::to_owned)
        .or_else(|| {
            std::env::var("SHELL")
                .ok()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "/bin/bash".to_owned());
    let python = python_interpreter()?;
    let mut argv: Vec<String> = vec![
        "exec".to_owned(),
        python,
        "-I".to_owned(),
        python_main()?,
        "--project-root".to_owned(),
        cfg.root.to_string_lossy().into_owned(),
    ];
    if box_mode {
        argv.push("box".to_owned());
        argv.push("--name".to_owned());
        argv.push(name.to_owned());
        argv.push("--cwd".to_owned());
        argv.push(
            box_cwd
                .as_deref()
                .unwrap_or(&cfg.root)
                .to_string_lossy()
                .into_owned(),
        );
        if let Some(scope) = writable {
            argv.push("--writable".to_owned());
            argv.push(scope.to_owned());
        }
    } else {
        argv.push("run".to_owned());
        argv.push(slot.unwrap_or_default().to_owned());
        argv.push("--slot-type".to_owned());
        argv.push(slot_type.to_owned());
    }
    if let Some(value) = isolation {
        argv.push("--isolation".to_owned());
        argv.push(value.to_owned());
    }
    if effective_command.is_empty() {
        let resolved = which(&shell_name)
            .map(|path| realpath(&path))
            .unwrap_or_else(|| realpath(Path::new(&shell_name)));
        argv.push("--".to_owned());
        argv.push(resolved);
        argv.push("-i".to_owned());
    } else {
        argv.push("--".to_owned());
        argv.append(&mut effective_command);
    }
    let line = crate::unpushed_state::shell_join(&argv[1..]);
    let line = format!("exec {line}");
    if !format_json {
        return Ok(format!("{line}\n"));
    }
    let config = crate::config_validate::load_config(root, machine_override)?;
    let isolation_out = isolation
        .map(str::to_owned)
        .or_else(|| {
            config
                .get("sandbox")
                .and_then(|sandbox| sandbox.get("isolation"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "userns".to_owned());
    let payload = if box_mode {
        json!({
            "command": line,
            "slot_path": box_cwd.as_deref().unwrap_or(&cfg.root).to_string_lossy(),
            "isolation": isolation_out,
            "box": name,
        })
    } else {
        let slot_path = slot_directory(&cfg, slot.unwrap_or_default(), slot_type)?;
        json!({
            "command": line,
            "slot_path": slot_path.to_string_lossy(),
            "isolation": isolation_out,
        })
    };
    Ok(format!(
        "{}\n",
        serde_json::to_string(&payload).unwrap_or_default()
    ))
}
