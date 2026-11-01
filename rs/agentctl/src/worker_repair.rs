//! Native Rust `repair` for one headless turn-runner worker.
//!
//! Ports `recreate_window` from the other edition: refuse unknown, TUI
//! and dead-runner workers; return `already_present` when the recorded
//! presentation still exists; otherwise break the runner's tmux pane
//! into its own window when one can be found, or launch a transcript
//! view window in the recorded backend. Fixture-tested with fake
//! `tmux`/`herdr` binaries; never run against a live runtime here.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Map, Value};

use crate::worker_reset::{load_rows, normalize_row, save_rows, valid_name};

type Result<T> = std::result::Result<T, String>;

fn tmux(args: &[&str]) -> (i32, String, String) {
    run("tmux", args)
}

fn run(program: &str, args: &[&str]) -> (i32, String, String) {
    match Command::new(program).args(args).output() {
        Ok(output) => (
            output.status.code().unwrap_or(127),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ),
        Err(_) => (127, String::new(), format!("{program}: not found")),
    }
}

fn herdr_bin() -> String {
    std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_owned())
}

fn herdr(args: &[&str]) -> Result<Value> {
    let (code, stdout, stderr) = run(&herdr_bin(), args);
    if code != 0 {
        let detail = {
            let text = stderr.trim();
            if text.is_empty() {
                let text = stdout.trim();
                if text.is_empty() {
                    format!("exit {code}")
                } else {
                    text.to_owned()
                }
            } else {
                text.to_owned()
            }
        };
        return Err(format!("herdr {} failed: {detail}", args.join(" ")));
    }
    let value: Value = serde_json::from_str(&stdout).map_err(|error| {
        format!(
            "herdr {} returned invalid API JSON: {error}",
            args.join(" ")
        )
    })?;
    Ok(value.get("result").cloned().unwrap_or(value))
}

fn herdr_not_found(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("not found") || lower.contains("no such") || lower.contains("not_found")
}

fn boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|text| text.trim().to_owned())
}

fn pid_start_time(pid: i64) -> Option<i64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 22 is starttime; the comm field may contain spaces/parens.
    let after = text.rsplit(')').next()?;
    let fields: Vec<&str> = after.split_whitespace().collect();
    // `after` starts at field 3, so field 22 is index 19.
    fields.get(19).and_then(|value| value.parse::<i64>().ok())
}

fn pid_ppid(pid: i64) -> Option<i64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = text.rsplit(')').next()?;
    let fields: Vec<&str> = after.split_whitespace().collect();
    fields.get(1).and_then(|value| value.parse::<i64>().ok())
}

fn is_descendant(ancestor: i64, pid: i64) -> bool {
    let mut current = pid;
    for _ in 0..64 {
        if current == ancestor {
            return true;
        }
        match pid_ppid(current) {
            Some(parent) if parent > 0 && parent != current => current = parent,
            _ => return false,
        }
    }
    false
}

fn row_text(row: &Map<String, Value>, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn started_text(row: &Map<String, Value>) -> Option<String> {
    match row.get("runner_started_at") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => None,
    }
}

/// The boot identity recorded in the `runner-identity.json` sidecar for
/// exactly this row, mirroring `_load_runner_boot_id`.
fn sidecar_boot_id(home: &Path, row: &Map<String, Value>) -> Option<String> {
    let name = row.get("name").and_then(Value::as_str)?;
    let created_at = row.get("created_at").and_then(Value::as_str)?;
    let pid = row.get("runner_pid").and_then(Value::as_i64)?;
    let text =
        std::fs::read_to_string(home.join("state").join(name).join("runner-identity.json")).ok()?;
    let raw: Value = serde_json::from_str(&text).ok()?;
    if raw.get("schema").and_then(Value::as_str) != Some("agentctl-headless-runner/v1")
        || raw.get("name").and_then(Value::as_str) != Some(name)
        || raw.get("created_at").and_then(Value::as_str) != Some(created_at)
        || raw.get("runner_pid").and_then(Value::as_i64) != Some(pid)
        || raw.get("runner_started_at").and_then(|value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }) != started_text(row)
    {
        return None;
    }
    raw.get("runner_boot_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn runner_alive(home: &Path, row: &Map<String, Value>) -> bool {
    let Some(pid) = row.get("runner_pid").and_then(Value::as_i64) else {
        return false;
    };
    if let Some(recorded_boot) = sidecar_boot_id(home, row) {
        if boot_id().as_deref() != Some(recorded_boot.as_str()) {
            return false;
        }
    }
    let Some(current) = pid_start_time(pid) else {
        return false;
    };
    match started_text(row) {
        None => true,
        Some(started) => current.to_string() == started,
    }
}

fn exact_tmux_target(target: &str) -> Result<String> {
    let Some((session, name)) = target.split_once(':') else {
        return Err(format!("invalid recorded tmux window target {target:?}"));
    };
    if session.is_empty() || name.is_empty() {
        return Err(format!("invalid recorded tmux window target {target:?}"));
    }
    Ok(format!("={session}:={name}"))
}

fn tmux_target_exists(target: &str) -> bool {
    let Some((session, name)) = target.split_once(':') else {
        return false;
    };
    if session.is_empty() || name.is_empty() {
        return false;
    }
    let (code, stdout, _) = tmux(&[
        "list-windows",
        "-t",
        &format!("={session}"),
        "-F",
        "#{window_name}",
    ]);
    code == 0 && stdout.lines().any(|line| line == name)
}

fn tmux_session_exists(session: &str) -> bool {
    tmux(&["has-session", "-t", &format!("={session}")]).0 == 0
}

fn launch_tmux_window(
    name: &str,
    cwd: &str,
    command: &str,
    session: Option<&str>,
) -> Result<String> {
    let session = session.unwrap_or("subagents");
    if !tmux_session_exists(session) {
        let (code, _, stderr) = tmux(&[
            "new-session",
            "-d",
            "-s",
            session,
            "-n",
            name,
            "-c",
            cwd,
            "bash",
            "-lc",
            command,
        ]);
        if code != 0 {
            return Err(stderr.trim().to_owned());
        }
    } else {
        let (code, _, stderr) = tmux(&[
            "new-window",
            "-t",
            &format!("={session}:"),
            "-n",
            name,
            "-c",
            cwd,
            "bash",
            "-lc",
            command,
        ]);
        if code != 0 {
            return Err(stderr.trim().to_owned());
        }
    }
    Ok(format!("{session}:{name}"))
}

fn find_runner_pane(row: &Map<String, Value>) -> Option<String> {
    let pid = row.get("runner_pid").and_then(Value::as_i64)?;
    let (code, stdout, _) = tmux(&["list-panes", "-a", "-F", "#{pane_id}\t#{pane_pid}"]);
    if code != 0 {
        return None;
    }
    for line in stdout.lines() {
        let mut parts = line.split('\t');
        let (Some(pane), Some(raw_pid)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(pane_pid) = raw_pid.parse::<i64>() else {
            continue;
        };
        if is_descendant(pane_pid, pid) {
            return Some(pane.to_owned());
        }
    }
    None
}

fn break_runner_pane(row: &Map<String, Value>, name: &str) -> Result<Option<String>> {
    let Some(pane) = find_runner_pane(row) else {
        return Ok(None);
    };
    let target = row_text(row, "tmux_target");
    let session = target.split(':').next().unwrap_or("subagents");
    exact_tmux_target(&target)?;
    let (code, _, stderr) = tmux(&[
        "break-pane",
        "-d",
        "-s",
        &pane,
        "-t",
        &format!("={session}:"),
        "-n",
        name,
    ]);
    if code != 0 {
        return Err(format!(
            "failed to break pane {pane} into window {name:?}: {}",
            stderr.trim()
        ));
    }
    Ok(Some(pane))
}

fn herdr_tab_exists(tab: &str) -> Result<bool> {
    match herdr(&["tab", "get", tab]) {
        Ok(_) => Ok(true),
        Err(error) if herdr_not_found(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn herdr_workspace_label() -> String {
    std::env::var("SUBAGENTS_HERDR_WORKSPACE").unwrap_or_else(|_| "subagents".to_owned())
}

fn herdr_workspace_id(home: &Path, cwd: &str) -> Result<String> {
    // Configured workspace from the backend config, when it still exists.
    let config_path = home.join("backend.json");
    let configured = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .get("herdr_workspace_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    if let Some(id) = configured {
        if let Ok(value) = herdr(&["workspace", "get", &id]) {
            if value
                .get("workspace")
                .and_then(|w| w.get("label"))
                .and_then(Value::as_str)
                == Some("subagents")
            {
                return Ok(id);
            }
        }
    }
    let listed = herdr(&["workspace", "list"])?;
    let remember = |id: &str| {
        let path = home.join("backend.json");
        let mut config: Map<String, Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        config.insert(
            "herdr_workspace_id".to_owned(),
            Value::String(id.to_owned()),
        );
        let _ = std::fs::write(
            &path,
            serde_json::to_string(&Value::Object(config)).unwrap_or_default(),
        );
    };
    let workspaces = listed
        .get("workspaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for workspace in &workspaces {
        if workspace.get("label").and_then(Value::as_str) == Some(herdr_workspace_label().as_str())
        {
            if let Some(id) = workspace.get("workspace_id").and_then(Value::as_str) {
                remember(id);
                return Ok(id.to_owned());
            }
        }
    }
    let created = herdr(&[
        "workspace",
        "create",
        "--cwd",
        cwd,
        "--label",
        "subagents",
        "--no-focus",
    ])?;
    let id = created
        .get("workspace")
        .and_then(|w| w.get("workspace_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Herdr did not return its new workspace id".to_owned())?;
    remember(&id);
    Ok(id)
}

fn launch_herdr_window(home: &Path, name: &str, cwd: &str, command: &str) -> Result<String> {
    let workspace = herdr_workspace_id(home, cwd)?;
    let created = herdr(&[
        "tab",
        "create",
        "--workspace",
        &workspace,
        "--cwd",
        cwd,
        "--label",
        "subagent",
        "--no-focus",
    ])?;
    let tab = created
        .get("tab")
        .and_then(|t| t.get("tab_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| "Herdr did not return a tab and root pane".to_owned())?
        .to_owned();
    let pane = created
        .get("root_pane")
        .and_then(|t| t.get("pane_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| "Herdr did not return a tab and root pane".to_owned())?
        .to_owned();
    let run_result = herdr(&[
        "pane",
        "run",
        &pane,
        &format!("/bin/bash -lc {}", shell_quote(command)),
    ])
    .and_then(|_| herdr(&["tab", "rename", &tab, name]));
    if let Err(error) = run_result {
        if herdr_tab_exists(&tab).unwrap_or(false) {
            let _ = herdr(&["tab", "close", &tab]);
        }
        return Err(error);
    }
    Ok(tab)
}

fn shell_quote(text: &str) -> String {
    // Mirrors shlex.quote: safe strings stay bare.
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

fn window_exists(row: &Map<String, Value>) -> Result<bool> {
    let backend = row_text(row, "backend");
    let target = row_text(row, "tmux_target");
    if backend == "tmux" {
        Ok(tmux_target_exists(&target))
    } else {
        Ok(!target.is_empty() && herdr_tab_exists(&target)?)
    }
}

fn transcript_path(home: &Path, name: &str) -> PathBuf {
    home.join("state").join(name).join("transcript.log")
}

fn last_message_preview(home: &Path, name: &str) -> String {
    let path = home.join("state").join(name).join("last-message.txt");
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= 240 {
        joined
    } else {
        let truncated: String = joined.chars().take(237).collect();
        format!("{truncated}...")
    }
}

fn write_event(home: &Path, name: &str, preview: &str) {
    let state = home.join("state");
    let _ = std::fs::create_dir_all(&state);
    let event = json!({
        "id": format!("{}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0), std::process::id()),
        "type": "presentation_restored",
        "name": name,
        "ts": chrono_now(),
        "preview": preview,
    });
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("events.jsonl"))
    {
        use std::io::Write as _;
        let _ = writeln!(
            file,
            "{}",
            serde_json::to_string(&event).unwrap_or_default()
        );
    }
}

fn chrono_now() -> String {
    // ISO-8601 UTC with seconds precision, from the system clock.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Recreate one worker's presentation in the runtime home at `home`;
/// returns the worker response document (`result` plus `record`).
pub fn recreate_window(home: &Path, name: &str) -> Result<Value> {
    if !valid_name(name) {
        return Err(format!(
            "invalid agent name {name:?}: use letters, digits, '-' or '_' (<=64 chars, must start alphanumeric)"
        ));
    }
    let (rows, _lock) = load_rows(home)?;
    let index = rows
        .iter()
        .position(|row| row.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| format!("unknown agent '{name}'"))?;
    let mut row = rows[index].clone();
    // The other edition's registry record carries the runner start
    // time as a string; mirror that in the record we return and save.
    if let Some(started) = row.get("runner_started_at").and_then(Value::as_i64) {
        row.insert(
            "runner_started_at".to_owned(),
            Value::String(started.to_string()),
        );
    }
    if row_text(&row, "mode") == "tui" {
        return Err(
            "recreating an interactive TUI would lose its visible conversation; inspect or restore its Herdr tab manually"
                .to_owned(),
        );
    }
    if !runner_alive(home, &row) {
        return Err(format!(
            "agent '{name}' runner pid {} is not alive or was reused",
            row.get("runner_pid")
                .map(|value| value.to_string())
                .unwrap_or_else(|| "None".to_owned())
        ));
    }
    let backend = row_text(&row, "backend");
    if window_exists(&row)? {
        return Ok(response(&row, "already_present", None, true));
    }
    let mut pane_id: Option<String> = None;
    if backend == "tmux" {
        pane_id = break_runner_pane(&row, name)?;
    }
    let mode = if pane_id.is_some() {
        "break_pane"
    } else {
        "transcript_view"
    };
    if pane_id.is_none() {
        let transcript = transcript_path(home, name);
        let command = format!(
            "echo '[subagents] presentation view for live runner pid {}'; echo '[subagents] input still goes through inbox/MCP, not this pane'; touch {}; tail -n +1 -F {}",
            row.get("runner_pid").and_then(Value::as_i64).unwrap_or(0),
            shell_quote(&transcript.to_string_lossy()),
            shell_quote(&transcript.to_string_lossy())
        );
        let launched = if backend == "tmux" {
            exact_tmux_target(&row_text(&row, "tmux_target"))?;
            let target = row_text(&row, "tmux_target");
            let session = target.split(':').next().unwrap_or("subagents").to_owned();
            launch_tmux_window(name, &row_text(&row, "cwd"), &command, Some(&session))
        } else {
            launch_herdr_window(home, name, &row_text(&row, "cwd"), &command)
        };
        let target = launched.map_err(|error| {
            format!("failed to create transcript-view window for '{name}': {error}")
        })?;
        row.insert("tmux_target".to_owned(), Value::String(target));
        let mut updated = rows.clone();
        updated[index] = normalize_row(&row);
        save_rows(home, &updated)?;
        row = updated[index].clone();
    }
    write_event(home, name, &last_message_preview(home, name));
    let alive = window_exists(&row)?;
    Ok(response(&row, mode, pane_id.as_deref(), alive))
}

fn response(row: &Map<String, Value>, mode: &str, pane_id: Option<&str>, alive: bool) -> Value {
    json!({
        "result": {
            "name": row_text(row, "name"),
            "mode": mode,
            "backend": row_text(row, "backend"),
            "tmux_target": row_text(row, "tmux_target"),
            "pane_id": pane_id,
            "window_alive": alive,
        },
        "record": row,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_tui_workers_are_refused() {
        let home = std::env::temp_dir().join(format!("worker-repair-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("registry.json"),
            serde_json::to_string(&json!([{
                "name": "worker", "mode": "tui", "backend": "herdr",
                "runner_pid": 1, "runner_started_at": null
            }]))
            .unwrap(),
        )
        .unwrap();
        assert!(recreate_window(&home, "ghost")
            .unwrap_err()
            .contains("unknown agent"));
        assert!(recreate_window(&home, "worker")
            .unwrap_err()
            .contains("interactive TUI"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
