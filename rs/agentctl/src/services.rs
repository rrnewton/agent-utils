//! `agentctl services`: build compiled snapshots and move a team's long-running agentctl
//! services onto them.
//!
//! Short commands can run from a source checkout through any launcher, but a chat bridge
//! (`chat run`), the agent watcher (`watch-agents`), and a headless agent's wrapper
//! (`run-headless`) load their binary once, restart on a crash, and start helpers (the Slack
//! plugin, the outbound helper) that must match it. They must run a fixed, compiled binary.
//! `services build` makes one from a clean `git archive` of a commit, `services list` shows which
//! binary each service runs, and `services refresh` moves them, one at a time, with a backup,
//! written rollback commands, a health check, and an automatic rollback.
//!
//! The root directory holds two collections, each in its own subdirectory:
//!
//! ```text
//! ROOT/snapshots/COMMIT12/          one compiled snapshot per commit (see `snapshot`)
//! ROOT/refreshes/STAMP-COMMIT12/    one directory per refresh run: UNIT.before, ROLLBACK.sh,
//!                                   report.json
//! ROOT/refresh.lock                 held while a refresh runs
//! ```

pub mod discover;
pub mod host;
pub mod refresh;
pub mod snapshot;
pub mod unit;

#[cfg(test)]
mod tests;

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde_json::{json, Value};

use discover::{Kind, OptionTables, Service};
use host::Host;
use snapshot::Snapshot;

/// The default root: `$XDG_STATE_HOME/agentctl/services`, else `~/.local/state/agentctl/services`.
#[must_use]
pub fn default_root() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })
        .map(|state| state.join("agentctl/services"))
}

/// The default drop-in directory: `$XDG_CONFIG_HOME/systemd/user`, else
/// `~/.config/systemd/user`.
#[must_use]
pub fn default_drop_in_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .map(|config| config.join("systemd/user"))
}

/// `YYYY-MM-DDTHH:MM:SSZ` for Unix seconds.
#[must_use]
pub fn utc(seconds: u64) -> String {
    crate::chat_runtime::utc_timestamp(seconds.saturating_mul(1000))
}

fn short(commit: &str) -> &str {
    commit.get(..8).unwrap_or(commit)
}

/// The outbound helper a bridge's state configures (`config.outbound_command.executable` in
/// `bridge.json`).
fn outbound_helper(host: &dyn Host, service: &Service) -> Option<PathBuf> {
    let bytes = host.read_file(&service.bridge_state.as_ref()?.join("bridge.json"))?;
    let document: Value = serde_json::from_slice(&bytes).ok()?;
    document["config"]["outbound_command"]["executable"]
        .as_str()
        .map(PathBuf::from)
}

/// What `services list` reports about one service.
fn describe(host: &dyn Host, service: &Service, newest: Option<&Snapshot>) -> Value {
    let commit = host.binary_commit(&service.binary);
    let running = service.state.main_pid.and_then(|pid| {
        let exe = host.process_exe(pid)?;
        if discover::is_agentctl_name(&exe.to_string_lossy()) {
            return Some(exe);
        }
        // A script without `exec` runs agentctl as the shell's child.
        host.process_children(pid)
            .into_iter()
            .filter_map(|child| host.process_exe(child))
            .find(|child| discover::is_agentctl_name(&child.to_string_lossy()))
            .or(Some(exe))
    });
    let running_text = running
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());
    // The kernel marks an executable replaced on disk since the process started.
    let running_matches = running_text
        .as_deref()
        .map(|text| discover::normalize(Path::new(text)) == discover::normalize(&service.binary));
    let plugin_home = service.plugin_home();
    let plugin = plugin_home
        .as_ref()
        .map(|home| home.join("plugins/slack/agentctl-slack-plugin"));
    let plugin_sha = plugin.as_ref().and_then(|path| host.file_sha256(path));
    let outbound = (service.kind == Kind::Bridge)
        .then(|| outbound_helper(host, service))
        .flatten();
    let outbound_sha = outbound.as_ref().and_then(|path| host.file_sha256(path));
    let newest_view = newest.map(|snapshot| {
        let on_snapshot = discover::normalize(&service.binary)
            == discover::normalize(&snapshot.agentctl())
            && (service.kind != Kind::Bridge
                || plugin_home.as_deref().map(discover::normalize)
                    == Some(discover::normalize(&snapshot.home())));
        json!({
            "commit": snapshot.commit,
            "path": snapshot.path,
            "on_snapshot": on_snapshot,
            "same_commit": commit.as_deref() == Some(snapshot.commit.as_str()),
            "plugin_matches": (service.kind == Kind::Bridge)
                .then(|| plugin_sha.is_some() && plugin_sha.as_deref() == snapshot.sha256(snapshot::PLUGIN)),
            "outbound_matches": (service.kind == Kind::Bridge)
                .then(|| outbound_sha.is_some() && outbound_sha.as_deref() == snapshot.sha256(snapshot::OUTBOUND)),
        })
    });
    json!({
        "unit": service.unit,
        "kind": service.kind.label(),
        "active_state": service.state.active_state,
        "main_pid": service.state.main_pid,
        "transient": service.state.transient,
        "fragment_path": service.state.fragment_path,
        "drop_in_paths": service.state.drop_in_paths,
        "binary": service.binary,
        "commit": commit,
        "running_binary": running_text,
        "running_binary_matches": running_matches,
        "registry": service.registry,
        "working_directory": service.working_directory,
        "agent": service.agent,
        "plugin_home": plugin_home,
        "plugin_sha256": plugin_sha,
        "outbound_helper": outbound,
        "outbound_sha256": outbound_sha,
        "state_dirs": service.state_dirs(),
        "log": service.log,
        "exec_start": service.exec.argv,
        "newest_snapshot": newest_view,
    })
}

/// The `services list` report: (JSON document, text).
pub fn list(
    host: &dyn Host,
    tables: &dyn OptionTables,
    registry: &Path,
    root: &Path,
) -> Result<(Value, String), String> {
    let (services, unreadable) = discover::discover(host, tables, registry)?;
    let snapshots = snapshot::list(root);
    let newest = snapshots.last();
    let rows: Vec<Value> = services
        .iter()
        .map(|service| describe(host, service, newest))
        .collect();
    let mut text = String::new();
    match newest {
        Some(snapshot) => {
            let _ = writeln!(
                text,
                "newest snapshot: {} built {} at {} ({} snapshot{} under {})",
                short(&snapshot.commit),
                utc(snapshot.built_at),
                snapshot.path.display(),
                snapshots.len(),
                if snapshots.len() == 1 { "" } else { "s" },
                snapshot::snapshots_dir(root).display()
            );
        }
        None => {
            let _ = writeln!(
                text,
                "no snapshot under {}; `agentctl services build` makes one",
                snapshot::snapshots_dir(root).display()
            );
        }
    }
    if rows.is_empty() {
        let _ = writeln!(
            text,
            "no systemd user unit runs agentctl chat run, watch-agents, or run-headless with registry {}",
            registry.display()
        );
    }
    for row in &rows {
        let _ = writeln!(
            text,
            "\n{}  {}  {}{}{}",
            row["unit"].as_str().unwrap_or("?"),
            row["kind"].as_str().unwrap_or("?"),
            row["active_state"].as_str().unwrap_or("?"),
            row["main_pid"]
                .as_u64()
                .map(|pid| format!(" pid {pid}"))
                .unwrap_or_default(),
            if row["transient"] == true {
                "  transient"
            } else {
                "  unit file"
            }
        );
        let commit = row["commit"].as_str().map_or("(reports no commit)", short);
        let _ = writeln!(
            text,
            "  binary    {}  commit {commit}",
            row["binary"].as_str().unwrap_or("?")
        );
        if row["running_binary_matches"] == false {
            let _ = writeln!(
                text,
                "  running   {} (not the unit's binary: restarted since, or replaced on disk)",
                row["running_binary"].as_str().unwrap_or("?")
            );
        }
        if let Some(agent) = row["agent"].as_str() {
            let _ = writeln!(text, "  agent     {agent}");
        }
        if let Some(home) = row["plugin_home"].as_str() {
            let _ = writeln!(text, "  plugins   {home}");
        }
        if let Some(helper) = row["outbound_helper"].as_str() {
            let _ = writeln!(text, "  outbound  {helper} (set in bridge.json)");
        }
        for directory in row["state_dirs"].as_array().into_iter().flatten() {
            let _ = writeln!(text, "  state     {}", directory.as_str().unwrap_or("?"));
        }
        if let Some(log) = row["log"].as_str() {
            let _ = writeln!(text, "  log       {log}");
        }
        let newest_view = &row["newest_snapshot"];
        if newest_view.is_object() {
            let verdict = if newest_view["on_snapshot"] == true {
                "on the newest snapshot".to_owned()
            } else if newest_view["same_commit"] == true {
                format!(
                    "same commit as the newest snapshot, but not run from it; refresh would move it to {}",
                    newest_view["path"].as_str().unwrap_or("?")
                )
            } else {
                format!(
                    "NOT on the newest snapshot ({}); refresh would move it",
                    newest_view["commit"].as_str().map_or("?", short)
                )
            };
            let _ = writeln!(text, "  snapshot  {verdict}");
            if newest_view["plugin_matches"] == false {
                let _ = writeln!(
                    text,
                    "            its Slack plugin differs from the snapshot's"
                );
            }
            if newest_view["outbound_matches"] == false {
                let _ = writeln!(
                    text,
                    "            its outbound helper differs from the snapshot's; refresh leaves it (it is part of the bridge's configuration)"
                );
            }
        }
    }
    for (unit, error) in &unreadable {
        let _ = writeln!(text, "\n{unit}: runs agentctl but cannot be read: {error}");
    }
    let document = json!({
        "schema": "agentctl-services-list/v1",
        "registry": registry,
        "root": root,
        "newest_snapshot": newest.map(|snapshot| json!({
            "commit": snapshot.commit,
            "path": snapshot.path,
            "built_at_utc": utc(snapshot.built_at),
        })),
        "snapshots": snapshots.len(),
        "services": rows,
        "unreadable": unreadable
            .iter()
            .map(|(unit, error)| json!({"unit": unit, "error": error}))
            .collect::<Vec<_>>(),
    });
    Ok((document, text))
}

/// Run `services build` and describe the snapshot as JSON.
pub fn build(host: &dyn Host, options: &snapshot::BuildOptions) -> Result<Value, String> {
    let (snapshot, built) = snapshot::build(host, options)?;
    Ok(json!({
        "schema": "agentctl-services-build/v1",
        "built": built,
        "commit": snapshot.commit,
        "path": snapshot.path,
        "agentctl": snapshot.agentctl(),
        "plugin_home": snapshot.home(),
        "built_at_utc": utc(snapshot.built_at),
        "files": snapshot.document["files"],
    }))
}

/// Run `services refresh` while holding `ROOT/refresh.lock`.
pub fn refresh(
    host: &dyn Host,
    tables: &dyn OptionTables,
    registry: &Path,
    root: &Path,
    options: &refresh::RefreshOptions,
) -> Result<refresh::RefreshReport, String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
        .map_err(|error| format!("{}: {error}", root.display()))?;
    let lock_path = root.join("refresh.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|error| format!("{}: {error}", lock_path.display()))?;
    if !options.dry_run {
        lock.try_lock_exclusive().map_err(|_| {
            format!(
                "another services refresh holds {}; wait for it to finish",
                lock_path.display()
            )
        })?;
        refresh::reap_check_units(host);
    }
    let (services, unreadable) = discover::discover(host, tables, registry)?;
    let mut report = refresh::refresh(host, &services, options)?;
    for (unit, error) in &unreadable {
        let _ = writeln!(
            report.text,
            "{unit}: runs agentctl but cannot be read, so it was left alone: {error}"
        );
    }
    report.document["unreadable"] = json!(unreadable
        .iter()
        .map(|(unit, error)| json!({"unit": unit, "error": error}))
        .collect::<Vec<_>>());
    Ok(report)
}
